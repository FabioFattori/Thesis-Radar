use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Write;
use std::time::Duration;

// ====== CONFIGURAZIONE ======
// Token dei board: sono quelli nell'URL careers, es. boards.greenhouse.io/<token>
// Non li ho verificati tutti: quelli errati vengono solo segnalati e saltati.
const GREENHOUSE: &[&str] = &["cloudflare", "databricks", "stripe", "datadog", "mongodb"];
const LEVER: &[&str] = &["spotify", "palantir"];

const INTERN_WORDS: &[&str] = &[
    "intern", "thesis", "working student", "werkstudent", "praktikum",
    "trainee", "master's thesis", "research student", "stage"
];
const TECH_WORDS: &[&str] = &[
    "software", "machine learning", "deep learning", "ai ", "data", "cloud",
    "distributed", "embedded", "security", "rust", "python", "c++", "robotics",
    "computer", "systems", "backend", "devops", "research", "engineer", "engineering"
];
const BAD_WORDS: &[&str] = &["sales", "marketing", "finance", "hr ", "legal", "recruit"];
const ITALY: &[&str] = &["italy", "italia", "milan", "milano", "rome", "roma", "turin", "torino", ", it"];
const MIN_SCORE: i32 = 2;
// ============================

#[derive(Debug, Serialize, Clone)]
struct Job {
    source: String,
    company: String,
    title: String,
    location: String,
    url: String,
    score: i32,
}

fn is_abroad(loc: &str) -> bool {
    let l = loc.to_lowercase();
    !ITALY.iter().any(|k| l.contains(k))
}

fn evaluate(title: &str, body: &str) -> Option<i32> {
    let t = title.to_lowercase();
    if !INTERN_WORDS.iter().any(|k| t.contains(k)) {
        return None;
    }
    let all = format!("{t} {}", body.to_lowercase());
    let mut s = 0;
    s += TECH_WORDS.iter().filter(|k| all.contains(*k)).count() as i32;
    s -= 3 * BAD_WORDS.iter().filter(|k| t.contains(*k)).count() as i32;
    s += if all.contains("master") { 2 } else { 0 };
    (s >= MIN_SCORE).then_some(s)
}

// ---------- AMAZON ----------
#[derive(Deserialize)]
struct AmazonResp {
    #[serde(default)] hits: usize,
    #[serde(default)] jobs: Vec<AmazonJob>,
}
#[derive(Deserialize)]
struct AmazonJob {
    #[serde(default)] title: String,
    #[serde(default)] job_path: String,
    #[serde(default)] normalized_location: Option<String>,
    #[serde(default)] description_short: Option<String>,
    #[serde(default)] basic_qualifications: Option<String>,
}

async fn amazon(c: &reqwest::Client) -> anyhow::Result<Vec<Job>> {
    let (mut out, mut offset, limit) = (vec![], 0usize, 50usize);
    loop {
        let r: AmazonResp = c
            .get("https://www.amazon.jobs/en/search.json")
            .query(&[
                ("base_query", "intern"),
                ("offset", &offset.to_string()),
                ("result_limit", &limit.to_string()),
            ])
            .send().await?.error_for_status()?.json().await?;
        if r.jobs.is_empty() { break; }
        for j in r.jobs {
            let loc = j.normalized_location.unwrap_or_default();
            let body = format!(
                "{} {}",
                j.description_short.unwrap_or_default(),
                j.basic_qualifications.unwrap_or_default()
            );
            if !is_abroad(&loc) { continue; }
            if let Some(score) = evaluate(&j.title, &body) {
                out.push(Job {
                    source: "amazon".into(), company: "Amazon".into(),
                    title: j.title, location: loc,
                    url: format!("https://www.amazon.jobs{}", j.job_path), score,
                });
            }
        }
        offset += limit;
        if offset >= r.hits { break; }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Ok(out)
}

// ---------- GREENHOUSE ----------
#[derive(Deserialize)]
struct GhResp { #[serde(default)] jobs: Vec<GhJob> }
#[derive(Deserialize)]
struct GhJob {
    #[serde(default)] title: String,
    #[serde(default)] absolute_url: String,
    #[serde(default)] location: Option<GhLoc>,
    #[serde(default)] content: Option<String>,
}
#[derive(Deserialize)]
struct GhLoc { #[serde(default)] name: String }

async fn greenhouse(c: &reqwest::Client, token: &str) -> anyhow::Result<Vec<Job>> {
    let url = format!("https://boards-api.greenhouse.io/v1/boards/{token}/jobs?content=true");
    let r: GhResp = c.get(url).send().await?.error_for_status()?.json().await?;
    Ok(r.jobs.into_iter().filter_map(|j| {
        let loc = j.location.map(|l| l.name).unwrap_or_default();
        if !is_abroad(&loc) { return None; }
        let score = evaluate(&j.title, &j.content.unwrap_or_default())?;
        Some(Job {
            source: "greenhouse".into(), company: token.into(),
            title: j.title, location: loc, url: j.absolute_url, score,
        })
    }).collect())
}

// ---------- LEVER ----------
#[derive(Deserialize)]
struct LvJob {
    #[serde(default)] text: String,
    #[serde(default, rename = "hostedUrl")] hosted_url: String,
    #[serde(default)] categories: LvCat,
    #[serde(default, rename = "descriptionPlain")] description: String,
}
#[derive(Deserialize, Default)]
struct LvCat { #[serde(default)] location: Option<String> }

async fn lever(c: &reqwest::Client, company: &str) -> anyhow::Result<Vec<Job>> {
    let url = format!("https://api.lever.co/v0/postings/{company}?mode=json");
    let r: Vec<LvJob> = c.get(url).send().await?.error_for_status()?.json().await?;
    Ok(r.into_iter().filter_map(|j| {
        let loc = j.categories.location.unwrap_or_default();
        if !is_abroad(&loc) { return None; }
        let score = evaluate(&j.text, &j.description)?;
        Some(Job {
            source: "lever".into(), company: company.into(),
            title: j.text, location: loc, url: j.hosted_url, score,
        })
    }).collect())
}

// ---------- ARBEITNOW (Europa) ----------
#[derive(Deserialize)]
struct AnResp { #[serde(default)] data: Vec<AnJob> }
#[derive(Deserialize)]
struct AnJob {
    #[serde(default)] title: String,
    #[serde(default)] company_name: String,
    #[serde(default)] url: String,
    #[serde(default)] location: String,
    #[serde(default)] description: String,
}

async fn arbeitnow(c: &reqwest::Client) -> anyhow::Result<Vec<Job>> {
    let mut out = vec![];
    for page in 1..=5 {
        let r: AnResp = c
            .get("https://www.arbeitnow.com/api/job-board-api")
            .query(&[("page", page.to_string())])
            .send().await?.error_for_status()?.json().await?;
        if r.data.is_empty() { break; }
        for j in r.data {
            if !is_abroad(&j.location) { continue; }
            if let Some(score) = evaluate(&j.title, &j.description) {
                out.push(Job {
                    source: "arbeitnow".into(), company: j.company_name,
                    title: j.title, location: j.location, url: j.url, score,
                });
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Ok(out)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .user_agent("thesis-internship-bot/0.1 (personal use)")
        .timeout(Duration::from_secs(30))
        .build()?;

    let mut all: Vec<Job> = vec![];
    let mut report = |name: &str, r: anyhow::Result<Vec<Job>>| match r {
        Ok(v) => { println!("[ok]   {name}: {} offerte pertinenti", v.len()); all.extend(v); }
        Err(e) => eprintln!("[err]  {name}: {e}"),
    };

    report("amazon", amazon(&client).await);
    for t in GREENHOUSE {
        report(&format!("greenhouse/{t}"), greenhouse(&client, t).await);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    for t in LEVER {
        report(&format!("lever/{t}"), lever(&client, t).await);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    report("arbeitnow", arbeitnow(&client).await);

    // dedup + ordinamento per punteggio
    let mut seen = HashSet::new();
    all.retain(|j| seen.insert(j.url.clone()));
    all.sort_by(|a, b| b.score.cmp(&a.score));

    let mut f = std::fs::File::create("jobs.jsonl")?;
    for j in &all {
        writeln!(f, "{}", serde_json::to_string(j)?)?;
    }

    println!("\n=== {} offerte salvate in jobs.jsonl ===", all.len());
    for j in all.iter().take(20) {
        println!("[{:>2}] {} @ {} ({})\n     {}", j.score, j.title, j.company, j.location, j.url);
    }
    Ok(())
}
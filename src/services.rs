use crate::{
    domain::{Action, State},
    storage,
};
use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::{
    process::{Child, Command, Stdio},
    time::Duration,
};

fn client() -> Result<Client> {
    Ok(Client::builder()
        .timeout(Duration::from_secs(180))
        .build()?)
}
fn endpoint(s: &State, path: &str) -> Result<String> {
    let u = url::Url::parse(&s.ollama_url)?;
    if !matches!(
        u.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    ) {
        bail!("Ollama must use a local loopback address");
    }
    Ok(format!("{}{path}", s.ollama_url.trim_end_matches('/')))
}
pub fn interpret(s: &State, text: &str) -> Result<Vec<Action>> {
    let now = chrono::Utc::now();
    let system=format!("You are Angela's local activity interpreter. Return JSON {{\"actions\":[...]}} only. Current time: {}; timezone: {}. Never invent past activity. Action shapes: {{\"kind\":\"start\",\"title\":\"Docs\"}}, {{\"kind\":\"stop\"}}, {{\"kind\":\"sleep\"}}, {{\"kind\":\"wake\"}}, {{\"kind\":\"log\",\"title\":\"Drawing\",\"start\":\"RFC3339\",\"end\":\"RFC3339\"}}, {{\"kind\":\"task\",\"title\":\"VI\",\"minutes\":60,\"deadline\":null}}, {{\"kind\":\"requirement\",\"title\":\"Cardio\",\"minutes\":60,\"weekly\":false}}. Use canonical existing titles when possible. Requirements: {}. Task queue: {}. Log retrospectives with exact intervals relative to now; timestamps must include offsets. Do not mark scheduled work as completed. A request to start work is start, not log.",now.to_rfc3339(),s.timezone,serde_json::to_string(&s.requirements)?,serde_json::to_string(&s.tasks)?);
    let v:Value=client()?.post(endpoint(s,"/api/chat")?).json(&json!({"model":s.ollama_model,"stream":false,"format":"json","options":{"temperature":0},"messages":[{"role":"system","content":system},{"role":"user","content":text}]})).send()?.error_for_status()?.json()?;
    let body: Value = serde_json::from_str(
        v["message"]["content"]
            .as_str()
            .context("Ollama returned no text")?,
    )?;
    let actions: Vec<Action> = serde_json::from_value(body["actions"].clone())?;
    crate::domain::validate(&actions, now)?;
    Ok(actions)
}
pub fn ollama_models(s: &State) -> Result<Vec<String>> {
    let v: Value = client()?
        .get(endpoint(s, "/api/tags")?)
        .send()?
        .error_for_status()?
        .json()?;
    Ok(v["models"]
        .as_array()
        .context("No model list")?
        .iter()
        .filter_map(|m| m["name"].as_str().map(String::from))
        .collect())
}
pub fn pull(s: &State) -> Result<()> {
    let v: Value = client()?
        .post(endpoint(s, "/api/pull")?)
        .json(&json!({"name":s.ollama_model,"stream":false}))
        .send()?
        .error_for_status()?
        .json()?;
    if let Some(e) = v["error"].as_str() {
        bail!("{e}");
    }
    Ok(())
}
fn output(mut c: Command) -> Result<String> {
    let o = c.output()?;
    if !o.status.success() {
        bail!(
            "{}",
            String::from_utf8_lossy(&o.stderr)
                .chars()
                .take(800)
                .collect::<String>()
        );
    }
    Ok(String::from_utf8(o.stdout)?)
}
pub fn voice_models(s: &State) -> Result<Value> {
    let mut c = Command::new(&s.omaspeak);
    c.args(["--list-models", "--json"]);
    serde_json::from_str(&output(c)?).context("Omaspeak did not return a model list")
}
pub fn open_voice(s: &State) -> Result<()> {
    Command::new(&s.omaspeak)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Omaspeak executable not found; set its full path in Settings")?;
    Ok(())
}
pub fn record() -> Result<Child> {
    let path = storage::directory()?.join("recording.wav");
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
    }
    Command::new("pw-record")
        .args(["--rate=16000", "--channels=1", "--format=s16"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Install PipeWire (pw-record) to record audio")
}
pub fn transcribe(s: &State, mut child: Child) -> Result<String> {
    // SIGINT lets PipeWire finalize the WAV header before Omaspeak reads it.
    let result = (|| {
        let status = Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()?;
        if !status.success() {
            bail!("Could not stop recorder");
        }
        child.wait()?;
        let mut c = Command::new(&s.omaspeak);
        c.arg("--transcribe-file")
            .arg(storage::directory()?.join("recording.wav"))
            .arg("--json");
        if !s.voice_model.is_empty() {
            c.args(["--model", &s.voice_model]);
        }
        let v: Value = serde_json::from_str(&output(c)?)?;
        Ok(v["text"]
            .as_str()
            .context("Omaspeak returned no transcript")?
            .to_owned())
    })();
    let _ = std::fs::remove_file(storage::directory()?.join("recording.wav"));
    result
}

pub fn plan(s: &State) -> Result<Vec<Action>> {
    let now = chrono::Utc::now();
    let unmet: Vec<_> = s
        .requirements
        .iter()
        .filter(|r| s.applicable(r, now))
        .map(|r| json!({"title":r.title,"remaining_minutes":s.remaining(r,now),"weekly":r.weekly}))
        .collect();
    let prompt = json!({"now":now,"timezone":s.timezone,"active":s.active(),"queue":s.queue(now),"requirements":unmet,"fixed_events":s.visible_events(),"tasks":s.tasks,"recent_sleep":s.sessions.iter().filter(|x|x.title.eq_ignore_ascii_case("Sleep")).rev().take(7).collect::<Vec<_>>()});
    let v:Value=client()?.post(endpoint(s,"/api/chat")?).json(&json!({"model":s.ollama_model,"stream":false,"format":"json","options":{"temperature":0},"messages":[{"role":"system","content":"Order the supplied flexible queue to plan work around fixed commitments and irregular sleep. Return JSON {\"order\":[\"exact activity title\",...]}. Use only queue titles. Meet minimum requirements, prioritize urgent deadlines, and balance Docs/game development. Never invent completed activity or a bedtime. Omitted work remains in the queue."},{"role":"user","content":prompt.to_string()}]})).send()?.error_for_status()?.json()?;
    let body: Value = serde_json::from_str(
        v["message"]["content"]
            .as_str()
            .context("Ollama returned no plan")?,
    )?;
    let order: Vec<String> = serde_json::from_value(body["order"].clone())?;
    let actions = vec![Action::Plan { order }];
    let mut check = s.clone();
    crate::domain::apply(&mut check, &actions, now)?;
    Ok(actions)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interpretation_uses_local_structured_actions() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("server");
        let address = server.server_addr();
        let worker = std::thread::spawn(move || {
            let mut r = server.recv().expect("request");
            assert_eq!(r.url(), "/api/chat");
            let mut body = String::new();
            r.as_reader().read_to_string(&mut body).expect("body");
            let v: Value = serde_json::from_str(&body).expect("JSON");
            assert_eq!(v["messages"][1]["content"], "I am going to sleep");
            assert_eq!(v["stream"], false);
            r.respond(tiny_http::Response::from_string(
                json!({"message":{"content":"{\"actions\":[{\"kind\":\"sleep\"}]}"}}).to_string(),
            ))
            .expect("response");
        });
        let s = State {
            ollama_url: format!("http://{address}"),
            ..State::default()
        };
        let actions = interpret(&s, "I am going to sleep").expect("interpretation");
        worker.join().expect("server joined");
        assert!(matches!(actions[0], Action::Sleep));
    }
    #[test]
    fn remote_ollama_endpoints_are_rejected() {
        let s = State {
            ollama_url: "https://example.com".into(),
            ..State::default()
        };
        assert!(endpoint(&s, "/api/chat").is_err());
    }
}

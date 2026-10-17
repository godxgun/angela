use crate::{
    domain::{Calendar, Event, State},
    storage,
};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Duration, TimeZone, Utc};
use rand::{distributions::Alphanumeric, Rng};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::{Duration as Wait, Instant};
const TOKEN: &str = "https://oauth2.googleapis.com/token";
#[derive(Deserialize, Serialize)]
struct Token {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: i64,
    #[serde(default)]
    expires_at: i64,
}
#[derive(Deserialize)]
struct Credentials {
    client_id: String,
    client_secret: String,
}
fn credentials(path: &str) -> Result<Credentials> {
    let v: Value = serde_json::from_slice(
        &std::fs::read(path)
            .context("Choose a Google Desktop app OAuth credentials JSON file in Settings")?,
    )?;
    serde_json::from_value(
        v.get("installed")
            .context("Use Desktop app credentials, not a Web application client")?
            .clone(),
    )
    .map_err(Into::into)
}
fn client() -> Result<Client> {
    Ok(Client::builder().timeout(Wait::from_secs(30)).build()?)
}
fn random() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(64)
        .map(char::from)
        .collect()
}
pub fn connect(path: &str) -> Result<()> {
    let creds = credentials(path)?;
    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|e| anyhow::anyhow!("Local sign-in listener: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .context("No loopback address")?
        .port();
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let state = random();
    let verifier = random();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = url::Url::parse("https://accounts.google.com/o/oauth2/v2/auth")?;
    url.query_pairs_mut().extend_pairs([
        ("client_id", creds.client_id.as_str()),
        ("redirect_uri", &redirect),
        ("response_type", "code"),
        ("scope", "https://www.googleapis.com/auth/calendar.readonly"),
        ("access_type", "offline"),
        ("prompt", "consent"),
        ("state", &state),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
    ]);
    webbrowser::open(url.as_str()).context("Could not open your browser")?;
    let until = Instant::now() + Wait::from_secs(180);
    while Instant::now() < until {
        let Some(request) = server.recv_timeout(Wait::from_millis(250))? else {
            continue;
        };
        let callback = url::Url::parse(&format!("http://localhost{}", request.url()))?;
        let params: std::collections::HashMap<_, _> = callback.query_pairs().into_owned().collect();
        if callback.path() != "/callback" || params.get("state") != Some(&state) {
            request.respond(
                tiny_http::Response::from_string("Invalid callback").with_status_code(400),
            )?;
            continue;
        }
        let code = match params.get("code") {
            Some(c) => c.clone(),
            None => {
                request.respond(tiny_http::Response::from_string(
                    "Sign-in cancelled. Return to Angela.",
                ))?;
                bail!("Google sign-in was cancelled");
            }
        };
        request.respond(tiny_http::Response::from_string(
            "Sign-in received. You can return to Angela.",
        ))?;
        let mut token: Token = client()?
            .post(TOKEN)
            .form(&[
                ("code", code.as_str()),
                ("client_id", &creds.client_id),
                ("client_secret", &creds.client_secret),
                ("redirect_uri", &redirect),
                ("grant_type", "authorization_code"),
                ("code_verifier", &verifier),
            ])
            .send()?
            .error_for_status()
            .context("Google token exchange failed")?
            .json()?;
        if token.refresh_token.is_empty() {
            bail!("Google did not return an offline refresh token; reconnect and grant consent");
        }
        token.expires_at = Utc::now().timestamp() + token.expires_in;
        storage::write("google-token.json", &token)?;
        return Ok(());
    }
    bail!("Google sign-in timed out after three minutes; try Connect again")
}
fn access(path: &str) -> Result<String> {
    let mut token: Token = storage::read("google-token.json")
        .context("Google is not connected. Use Settings → Connect Google")?;
    if token.expires_at <= Utc::now().timestamp() + 60 {
        let creds = credentials(path)?;
        let mut fresh: Token = client()?
            .post(TOKEN)
            .form(&[
                ("client_id", &creds.client_id),
                ("client_secret", &creds.client_secret),
                ("refresh_token", &token.refresh_token),
                ("grant_type", &"refresh_token".to_owned()),
            ])
            .send()?
            .error_for_status()
            .context("Google session expired or access revoked; reconnect in Settings")?
            .json()?;
        if fresh.refresh_token.is_empty() {
            fresh.refresh_token = token.refresh_token;
        }
        fresh.expires_at = Utc::now().timestamp() + fresh.expires_in;
        storage::write("google-token.json", &fresh)?;
        token = fresh;
    }
    Ok(token.access_token)
}
fn pages(http: &Client, token: &str, url: &str, params: &[(&str, String)]) -> Result<Vec<Value>> {
    let mut out = vec![];
    let mut page = String::new();
    loop {
        let mut q = params.to_vec();
        if !page.is_empty() {
            q.push(("pageToken", page));
        }
        let v: Value = http
            .get(url)
            .bearer_auth(token)
            .query(&q)
            .send()?
            .error_for_status()?
            .json()?;
        out.extend(
            v["items"]
                .as_array()
                .context("Invalid Calendar API response")?
                .iter()
                .cloned(),
        );
        match v["nextPageToken"].as_str() {
            Some(s) => page = s.into(),
            None => break,
        }
    }
    Ok(out)
}
fn event_time(s: &State, v: &Value) -> Result<(DateTime<Utc>, bool)> {
    if let Some(d) = v["dateTime"].as_str() {
        return Ok((DateTime::parse_from_rfc3339(d)?.with_timezone(&Utc), false));
    }
    Ok((
        s.midnight(v["date"].as_str().context("Missing event time")?.parse()?),
        true,
    ))
}
pub fn sync(s: &State, week: DateTime<Utc>) -> Result<(Vec<Calendar>, Vec<Event>)> {
    let token = access(&s.google_credentials)?;
    let http = client()?;
    let list = pages(
        &http,
        &token,
        "https://www.googleapis.com/calendar/v3/users/me/calendarList",
        &[],
    )?;
    let calendars: Vec<_> = list
        .iter()
        .filter_map(|v| {
            let id = v["id"].as_str()?.to_owned();
            Some(Calendar {
                title: v["summary"].as_str().unwrap_or(&id).into(),
                visible: s
                    .calendars
                    .iter()
                    .find(|c| c.id == id)
                    .is_none_or(|c| c.visible),
                id,
            })
        })
        .collect();
    let mut events = vec![];
    for c in &calendars {
        let mut url = url::Url::parse("https://www.googleapis.com/calendar/v3/calendars/")?;
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("Invalid Calendar URL"))?
            .pop_if_empty()
            .push(&c.id)
            .push("events");
        let rows = pages(
            &http,
            &token,
            url.as_str(),
            &[
                ("timeMin", (week - Duration::days(1)).to_rfc3339()),
                ("timeMax", (week + Duration::days(8)).to_rfc3339()),
                ("singleEvents", "true".into()),
                ("maxResults", "2500".into()),
            ],
        )?;
        for v in rows {
            if v["status"] == "cancelled" {
                continue;
            }
            let (start, all_day) = event_time(s, &v["start"])?;
            let (end, _) = event_time(s, &v["end"])?;
            let title = v["summary"].as_str().unwrap_or("Untitled event");
            let deadline = deadline(
                s,
                title,
                v["description"].as_str().unwrap_or_default(),
                start,
                end,
                all_day,
            );
            events.push(Event {
                id: v["id"].as_str().unwrap_or_default().into(),
                title: v["summary"].as_str().unwrap_or("Untitled event").into(),
                start,
                end,
                all_day,
                calendar: c.id.clone(),
                deadline,
            });
        }
    }
    events.sort_by_key(|e| e.start);
    Ok((calendars, events))
}
pub fn disconnect() -> Result<()> {
    let p = storage::directory()?.join("google-token.json");
    if p.exists() {
        std::fs::remove_file(p)?;
    }
    Ok(())
}

fn deadline(
    s: &State,
    title: &str,
    description: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    all_day: bool,
) -> Option<DateTime<Utc>> {
    let title = title.to_lowercase();
    if !["entrega", "deadline", "submission", "due:"]
        .iter()
        .any(|word| title.contains(word))
    {
        return None;
    }
    if !all_day {
        return Some(start);
    }
    if let Some((_, after)) = description.to_lowercase().split_once("hora limite:") {
        if let Some(word) = after.split_whitespace().next() {
            if let Ok(time) = chrono::NaiveTime::parse_from_str(
                word.trim_matches(|c: char| !c.is_ascii_digit() && c != ':'),
                "%H:%M",
            ) {
                return s
                    .timezone
                    .from_local_datetime(
                        &start.with_timezone(&s.timezone).date_naive().and_time(time),
                    )
                    .earliest()
                    .map(|t| t.with_timezone(&Utc));
            }
        }
    }
    Some(end - Duration::minutes(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn university_deadlines_use_description_time() {
        let s = State::default();
        let start = "2026-10-18T23:00:00Z".parse().expect("start");
        let end = "2026-10-19T23:00:00Z".parse().expect("end");
        assert_eq!(
            deadline(
                &s,
                "Entrega de trabalhos",
                "Hora limite: 14:00",
                start,
                end,
                true
            )
            .expect("deadline")
            .to_rfc3339(),
            "2026-10-19T13:00:00+00:00"
        );
        assert!(deadline(&s, "Lecture", "", start, end, true).is_none());
    }
    #[test]
    fn all_day_dates_use_lisbon_midnight_and_offset_times_remain_exact() {
        let s = State::default();
        let (t, all) = event_time(&s, &json!({"date":"2026-10-19"})).expect("all day");
        assert!(all);
        assert_eq!(t.to_rfc3339(), "2026-10-18T23:00:00+00:00");
        let (t, all) =
            event_time(&s, &json!({"dateTime":"2026-10-19T10:00:00+01:00"})).expect("timed");
        assert!(!all);
        assert_eq!(t.to_rfc3339(), "2026-10-19T09:00:00+00:00");
    }
    #[test]
    fn calendar_pagination_keeps_query_and_encodes_page_token() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("server");
        let address = server.server_addr();
        let worker = std::thread::spawn(move || {
            let r = server.recv().expect("first request");
            assert!(r.url().contains("singleEvents=true"));
            assert!(
                r.headers()
                    .iter()
                    .any(|h| h.field.equiv("Authorization")
                        && h.value.as_str() == "Bearer test-token")
            );
            r.respond(tiny_http::Response::from_string(
                r#"{"items":[{"id":"a"}],"nextPageToken":"next+page"}"#,
            ))
            .expect("first response");
            let r = server.recv().expect("second request");
            assert!(r.url().contains("singleEvents=true"));
            assert!(r.url().contains("pageToken=next%2Bpage"));
            r.respond(tiny_http::Response::from_string(
                r#"{"items":[{"id":"b"}]}"#,
            ))
            .expect("second response");
        });
        let items = pages(
            &client().expect("client"),
            "test-token",
            &format!("http://{address}/events"),
            &[("singleEvents", "true".into())],
        )
        .expect("pages");
        worker.join().expect("server joined");
        assert_eq!(items.len(), 2);
        assert_eq!(items[1]["id"], "b");
    }
}

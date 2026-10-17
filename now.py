#!/usr/bin/env python3
"""now: local, adaptive activity journal and calendar-aware next-action CLI (MIT)."""
from __future__ import annotations

import argparse
import csv
import io
import json
import hashlib
import os
import re
import select
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request
from datetime import date, datetime, timedelta, time
from pathlib import Path
from zoneinfo import ZoneInfo

TZ = ZoneInfo(os.getenv("NOW_TZ", "Europe/Lisbon"))
DB_PATH = Path(os.getenv("NOW_DB", str(Path(os.getenv("XDG_DATA_HOME", str(Path.home() / ".local/share"))) / "now-local" / "now.sqlite3")))
MODEL = os.getenv("NOW_MODEL", "qwen2.5:1.5b")
PROJECTS = ("DI", "DJ", "VI", "DS")
CANONICAL = {"docs": "Docs", "docgun": "Docs", "docs.godgun.net": "Docs", "games": "Game development",
             "game": "Game development", "game dev": "Game development", "gamedev": "Game development",
             "art": "Drawing", "draw": "Drawing", "drawing": "Drawing", "gym": "Weightlifting",
             "lift": "Weightlifting", "lifting": "Weightlifting", "workout": "Weightlifting",
             "running": "Cardio", "run": "Cardio", "jog": "Cardio", "sleeping": "Sleep"}


def current_time() -> datetime:
    return datetime.now(TZ)


def stamp(t: datetime) -> str:
    return t.astimezone(TZ).isoformat(timespec="seconds")


def dt(s: str) -> datetime:
    parsed = datetime.fromisoformat(s)
    return parsed.replace(tzinfo=TZ) if parsed.tzinfo is None else parsed.astimezone(TZ)


def canonical(name: str) -> str:
    key = name.lower().strip()
    return CANONICAL.get(key, key.upper() if key in ("di", "dj", "vi", "ds") else name.strip())


def db() -> sqlite3.Connection:
    DB_PATH.parent.mkdir(parents=True, exist_ok=True)
    c = sqlite3.connect(DB_PATH)
    c.row_factory = sqlite3.Row
    c.executescript("""
        CREATE TABLE IF NOT EXISTS sessions (
          id INTEGER PRIMARY KEY, project TEXT NOT NULL, started TEXT NOT NULL,
          ended TEXT, note TEXT NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS journal (
          id INTEGER PRIMARY KEY, created TEXT NOT NULL, raw TEXT NOT NULL,
          source TEXT NOT NULL DEFAULT 'text'
        );
        CREATE TABLE IF NOT EXISTS journal_items (
          id INTEGER PRIMARY KEY, journal_id INTEGER NOT NULL,
          project TEXT NOT NULL, minutes INTEGER, detail TEXT NOT NULL DEFAULT '',
          FOREIGN KEY(journal_id) REFERENCES journal(id)
        );
        CREATE TABLE IF NOT EXISTS deadlines (
          id INTEGER PRIMARY KEY, title TEXT NOT NULL, due TEXT NOT NULL,
          done INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS settings (
          key TEXT PRIMARY KEY, value TEXT NOT NULL
        );
    """)
    return c


def active_session(c):
    return c.execute("SELECT * FROM sessions WHERE ended IS NULL ORDER BY id DESC LIMIT 1").fetchone()


def duration_minutes(start: str, end: datetime | None = None) -> int:
    return max(0, round(((end or current_time()) - dt(start)).total_seconds() / 60))


def add_deadline(c, title: str, when: str):
    due = dt(when)
    c.execute("INSERT INTO deadlines(title,due) VALUES (?,?)", (title, stamp(due)))
    c.commit()
    print(f"Added deadline: {title} — {due:%a %d %b %H:%M}")


def deadlines(c, include_done=False):
    query = "SELECT * FROM deadlines" + ("" if include_done else " WHERE done = 0") + " ORDER BY due"
    return c.execute(query).fetchall()


def recent_totals(c, now=None):
    now = now or current_time()
    day_start = now.date() - timedelta(days=now.weekday())
    start_week = datetime.combine(day_start, time.min, tzinfo=TZ)
    start_day = datetime.combine(now.date(), time.min, tzinfo=TZ)
    stats = {"week": {}, "today": {}}
    for row in c.execute("SELECT * FROM sessions WHERE started >= ? OR ended IS NULL", (stamp(start_week),)):
        a = max(dt(row["started"]), start_week)
        b = dt(row["ended"]) if row["ended"] else now
        if b <= a:
            continue
        week = max(0, (b - a).total_seconds() / 60)
        day = max(0, (b - max(a, start_day)).total_seconds() / 60)
        key = row["project"]
        stats["week"][key] = stats["week"].get(key, 0) + week
        stats["today"][key] = stats["today"].get(key, 0) + day
    # Voice or text reports without a verified start time remain estimates, shown separately.
    estimates = {}
    reported_today = {}
    for r in c.execute("""SELECT i.project, i.minutes, j.created FROM journal_items i
                               JOIN journal j ON j.id=i.journal_id
                               WHERE j.created >= ? AND i.minutes IS NOT NULL""", (stamp(start_week),)):
        estimates[r["project"]] = estimates.get(r["project"], 0) + r["minutes"]
        if dt(r["created"]) >= start_day:
            reported_today[r["project"]] = reported_today.get(r["project"], 0) + r["minutes"]
    stats["reported_week"] = estimates
    stats["reported_today"] = reported_today
    return stats


def record_start(c, project: str, at=None):
    project = canonical(project)
    ongoing = active_session(c)
    at = at or current_time()
    if ongoing:
        if ongoing['project'] == project:
            print(f"Already tracking {project} since {dt(ongoing['started']):%H:%M}.")
            return
        # Starting another project (including sleep) ends the previous session.
        c.execute("UPDATE sessions SET ended=? WHERE id=?", (stamp(at), ongoing['id']))
        print(f"Switched from {ongoing['project']} after {duration_minutes(ongoing['started'], at)} min.")
    c.execute("INSERT INTO sessions(project,started) VALUES (?,?)", (project, stamp(at)))
    c.commit()
    print(f"Started: {project} at {at:%H:%M}")


def record_stop(c, expected=None):
    active = active_session(c)
    if not active:
        print("Nothing is currently being tracked.")
        return
    if expected is not None and active["project"] != expected:
        print(f"Tracking {active['project']}, not {expected}. Stop that session first.")
        return
    now = current_time()
    c.execute("UPDATE sessions SET ended = ? WHERE id = ?", (stamp(now), active["id"]))
    c.commit()
    print(f"Stopped: {active['project']} — {duration_minutes(active['started'], now)} min")


def parse_gcal_date(s):
    s = s.strip()
    for fmt in ("%Y-%m-%d", "%Y/%m/%d", "%m/%d/%Y", "%d/%m/%Y", "%b %d, %Y", "%d %b %Y"):
        try:
            return datetime.strptime(s, fmt).date()
        except ValueError:
            pass
    return None


def parse_gcal_time(day: date, s: str) -> datetime | None:
    s = s.strip()
    for fmt in ("%H:%M", "%I:%M%p", "%I:%M %p", "%H:%M:%S"):
        try:
            x = datetime.strptime(s, fmt)
            return datetime.combine(day, x.time(), tzinfo=TZ)
        except ValueError:
            pass
    return None


def parse_agenda_tsv(s: str):
    """Return normalized event records from gcalcli's documented TSV fields."""
    rows = []
    for row in csv.reader(io.StringIO(s), delimiter="\t"):
        if len(row) < 6:
            continue
        d = parse_gcal_date(row[0])
        if d is None:
            continue
        start = parse_gcal_time(d, row[1])
        title = row[5].strip()
        # gcalcli represents an all-day event with an empty start time.
        if not title:
            continue
        detail = " ".join(row[7:]) if len(row) >= 8 else ""
        if start is None:
            # A due-time embedded in an all-day university event, e.g. Hora limite: 14:00.
            match = re.search(r"Hora\s+limite\s*:\s*(\d{1,2}:\d{2})", detail, re.I)
            start = parse_gcal_time(d, match.group(1)) if match else datetime.combine(d, time.min, tzinfo=TZ)
        end_day = parse_gcal_date(row[2]) or d
        end = parse_gcal_time(end_day, row[3]) if row[3].strip() else None
        rows.append({"when": start, "end": end, "title": title, "all_day": not row[1].strip(), "details": detail})
    return sorted(rows, key=lambda r: r["when"])


def google_agenda(now=None, days=14):
    if not shutil.which("gcalcli"):
        return [], "gcalcli is not installed (optional Google Calendar integration)."
    now = now or current_time()
    end = now + timedelta(days=days)
    try:
        # gcalcli 4.x uses agenda-scoped TSV flags; earlier 3.x accepted global flags.
        commands = [
            ["gcalcli", "--nocolor", "agenda", now.strftime("%Y-%m-%d"), end.strftime("%Y-%m-%d"), "--tsv", "--details", "description"],
            ["gcalcli", "--nocolor", "--tsv", "--details", "description", "agenda", now.strftime("%Y-%m-%d"), end.strftime("%Y-%m-%d")],
        ]
        r = None
        for command in commands:
            r = subprocess.run(command, capture_output=True, text=True, timeout=30)
            if r.returncode == 0:
                break
        if r is None or r.returncode != 0:
            return [], "gcalcli error: " + (r.stderr.strip() or r.stdout.strip())[:250]
        rows = parse_agenda_tsv(r.stdout)
        return rows, None if rows else "Google Calendar returned no parseable events. Try `gcalcli agenda` to verify authentication."
    except (subprocess.TimeoutExpired, OSError) as e:
        return [], f"Google Calendar unavailable: {e}"


def cached_agenda(c, now=None, days=14, max_age_minutes=10):
    """Cache the read-only calendar bridge so a polling bar isn't a Google API client."""
    now = now or current_time()
    cache_key = f"calendar_cache_{days}"
    record = c.execute("SELECT value FROM settings WHERE key=?", (cache_key,)).fetchone()
    previous = None
    if record:
        try:
            saved = json.loads(record["value"])
            previous = ([{**e, "when": dt(e["when"]),
                          "end": dt(e["end"]) if e.get("end") else None}
                         for e in saved["events"]], dt(saved["at"]))
            if (now - previous[1]).total_seconds() < max_age_minutes * 60:
                return previous[0], None
        except (ValueError, TypeError, KeyError):
            previous = None
    events, err = google_agenda(now=now, days=days)
    if err is None:
        data = {"at": stamp(now), "events": [{**e, "when": stamp(e["when"]),
                "end": stamp(e["end"]) if e.get("end") else None} for e in events]}
        c.execute("INSERT OR REPLACE INTO settings(key,value) VALUES (?,?)",
                  (cache_key, json.dumps(data, ensure_ascii=False)))
        c.commit()
        return events, None
    # If the network briefly fails, display a clearly-marked cache, never claim it's live.
    if previous and (now - previous[1]) < timedelta(hours=12):
        return previous[0], f"Calendar stale since {previous[1]:%H:%M}: {err}"
    return events, err


def structured_recommendation(c, now, agenda):
    """Deterministic decisions. Ollama parses retrospective reports, not deadlines."""
    active = active_session(c)
    if active and active["project"] == "Sleep":
        return {"title": "Rest", "short": "Sleeping", "project": None,
                "reason": "Sleep is being tracked. The queue resumes when you wake."}

    active_fixed = next((e for e in agenda if fixed_event(e) and e.get("end")
                         and e["when"] <= now < e["end"]
                         and not any(k in e["title"].lower() for k in ("entrega", "deadline"))), None)
    if active_fixed:
        return {"title": active_fixed["title"], "short": "In class / event", "project": None,
                "reason": "A fixed calendar commitment is underway. Don't schedule over it."}

    next_fixed = next((e for e in agenda if fixed_event(e) and e["when"] > now
                       and not any(k in e["title"].lower() for k in ("entrega", "deadline"))), None)
    if next_fixed and next_fixed["when"] <= now + timedelta(minutes=20):
        return {"title": "Get ready: " + next_fixed["title"], "short": "Upcoming class",
                "project": None, "reason": f"Begins at {next_fixed['when']:%H:%M}, in less than 20 minutes."}

    due_list = [(dt(d["due"]), d["title"]) for d in deadlines(c)
                if dt(d["due"]) >= now]
    due_list += [(e["when"], e["title"] +
                  (": " + e["details"][:80] if e.get("details") else ""))
                 for e in agenda if e["when"] >= now and
                 any(k in e["title"].lower() for k in ("entrega", "deadline", "submission"))]
    if due_list:
        when, title = min(due_list, key=lambda item: item[0])
        hours = (when - now).total_seconds() / 3600
        if hours < 72:
            match = re.search(r"\b(DI|DJ|VI|DS|AMIII)\b", title, re.I)
            project = match.group(1).upper() if match else None
            return {"title": "Prepare: " + title, "short": f"Deadline {hours:.0f}h",
                    "project": project,
                    "reason": f"Due {when:%a %d %b %H:%M} ({hours:.1f}h). Prioritize before optional work."}

    totals = recent_totals(c, now)
    today = {p: max(totals["today"].get(p, 0), totals["reported_today"].get(p, 0))
             for p in set(totals["today"]) | set(totals["reported_today"])}
    for project in PROJECTS:
        remaining = round(60 - today.get(project, 0))
        if remaining > 0:
            return {"title": f"{project} — university project", "short": f"{project} · {remaining}m",
                    "project": project, "reason": f"{remaining} minutes until today's one-hour target."}
    exercise = "Weightlifting" if now.weekday() in (0, 2, 4) else "Cardio"
    remaining = round(60 - today.get(exercise, 0))
    if remaining > 0:
        return {"title": exercise, "short": f"{exercise} · {remaining}m", "project": exercise,
                "reason": f"{remaining} minutes until today's training goal."}
    remaining = round(120 - today.get("Drawing", 0))
    if remaining > 0:
        return {"title": "Drawing", "short": f"Drawing · {remaining}m", "project": "Drawing",
                "reason": f"{remaining} minutes until today's two-hour art practice goal."}
    week = {p: max(totals["week"].get(p, 0), totals["reported_week"].get(p, 0))
            for p in set(totals["week"]) | set(totals["reported_week"])}
    a, b = week.get("Docs", 0), week.get("Game development", 0)
    project = "Docs" if a <= b else "Game development"
    return {"title": project, "short": project,
            "project": project, "reason": f"Balance weekly work 50/50. Docs {a/60:.1f}h; games {b/60:.1f}h."}


def snapshot(c, now=None):
    """Stable JSON contract for Quickshell/Waybar; safe to call every minute."""
    now = now or current_time()
    agenda, calendar_error = cached_agenda(c, now, days=14)
    active = active_session(c)
    rec = structured_recommendation(c, now, agenda)
    local_due = [(dt(d["due"]), d["title"]) for d in deadlines(c)
                 if dt(d["due"]) >= now]
    remote_due = [(e["when"], e["title"] +
                   (" — " + e.get("details", "")[:90] if e.get("details") else ""))
                  for e in agenda if e["when"] >= now and
                  any(k in e["title"].lower() for k in ("entrega", "deadline", "submission"))]
    due_list = sorted(local_due + remote_due)
    hard = sorted((e for e in agenda if fixed_event(e) and e["when"] >= now
                   and not any(k in e["title"].lower() for k in ("entrega", "deadline", "submission"))),
                  key=lambda e: e["when"])
    totals = recent_totals(c, now)
    return {"generated_at": stamp(now),
            "active": ({"project": active["project"],
                        "minutes": duration_minutes(active["started"], now)} if active else None),
            "recommendation": rec,
            "nearest_deadline": ({"title": due_list[0][1],
                                  "when": due_list[0][0].strftime("%a %d %b %H:%M"),
                                  "hours_left": round((due_list[0][0] - now).total_seconds() / 3600, 1)}
                                 if due_list else None),
            "next_fixed": ({"title": hard[0]["title"],
                            "when": hard[0]["when"].strftime("%a %d %b %H:%M")}
                           if hard else None),
            "today_minutes": {p: round(max(totals["today"].get(p, 0),
                                               totals["reported_today"].get(p, 0)))
                              for p in set(totals["today"]) | set(totals["reported_today"])},
            "calendar_error": calendar_error}

def fixed_event(event):
    title = event["title"].lower()
    return any(k in title for k in ("aula", "defesa", "avaliação", "exame", "exam", "teste", "meeting", "appointment", "consulta", "deadline", "entrega"))


def priority(c, now, agenda):
    # Explicit, uncompleted deadlines take precedence over discretionary tasks.
    nearest = None
    for d in deadlines(c):
        if dt(d["due"]) >= now - timedelta(hours=24):
            nearest = {"title": d["title"], "when": dt(d["due"]), "source": "local"}
            break
    for event in agenda:
        title = event["title"].lower()
        if any(k in title for k in ("entrega", "deadline", "due", "submission")) and event["when"] >= now:
            if nearest is None or event["when"] < nearest["when"]:
                nearest = {"title": event["title"], "when": event["when"], "source": "calendar"}
    if nearest and nearest["when"] <= now + timedelta(hours=72):
        h = (nearest["when"] - now).total_seconds() / 3600
        return f"Deadline prep: {nearest['title']} ({max(0,h):.1f}h remaining)."

    stats = recent_totals(c, now)
    # Take the higher of timed measurements and reported estimates, never their sum.
    today = {p: max(stats["today"].get(p, 0), stats["reported_today"].get(p, 0))
             for p in set(stats["today"]) | set(stats["reported_today"])}
    # Four one-hour daily university project blocks; skip when logged time suffices.
    for p in PROJECTS:
        if today.get(p, 0) < 60:
            return f"{p} project: {max(0, round(60 - today.get(p, 0)))} minutes remaining on today's 1h target."
    exercise = "Weightlifting" if now.weekday() in (0, 2, 4) else "Cardio"
    if today.get(exercise, 0) < 60:
        return f"{exercise}: {max(0, round(60 - today.get(exercise, 0)))} minutes remaining."
    if today.get("Drawing", 0) < 120:
        return f"Drawing: {max(0, round(120 - today.get('Drawing', 0)))} minutes remaining."
    week = {p: max(stats["week"].get(p, 0), stats["reported_week"].get(p, 0))
            for p in set(stats["week"]) | set(stats["reported_week"])}
    a, b = week.get("Docs", 0), week.get("Game development", 0)
    target = "Docs" if a <= b else "Game development"
    return f"{target}: your less-worked side of the 50/50 split this week (Docs {a/60:.1f}h, games {b/60:.1f}h)."


def show_now(c):
    now = current_time()
    agenda, error = cached_agenda(c, now)
    print(f"\nNOW  {now:%A, %d %B %Y  %H:%M}  ({TZ.key})")
    print("─" * 57)
    active = active_session(c)
    if active:
        print(f"TRACKING: {active['project']} ({duration_minutes(active['started'], now)} min)")
        if active["project"] == "Sleep":
            print("Sleep session is open. Run `now wake` when you're awake; no work is assumed.")
    else:
        print("TRACKING: Nothing active — `now start Docs` to begin.")
    print()
    own = [d for d in deadlines(c) if dt(d["due"]) >= now - timedelta(days=1)]
    remote_due = [e for e in agenda if any(w in e["title"].lower() for w in ("entrega", "deadline", "submission")) and e["when"] >= now]
    if own or remote_due:
        print("DEADLINES")
        due_items = [(dt(d["due"]), d["title"]) for d in own] + [(e["when"], e["title"]) for e in remote_due]
        seen = set()
        for when, title in sorted(due_items)[:8]:
            key = (when.date(), title.casefold())
            if key in seen:
                continue
            seen.add(key)
            print(f"  {when:%a %d %b %H:%M}  {title}")
        print()
    hard = [e for e in agenda if fixed_event(e) and e["when"] >= now and not (e["all_day"] and not any(x in e["title"].lower() for x in ("entrega", "deadline")))]
    if hard:
        print("NEXT FIXED CALENDAR EVENT")
        for e in hard[:3]:
            print(f"  {e['when']:%a %H:%M}  {e['title']}")
        print()
    print("SUGGESTED NEXT ACTION")
    if active and active["project"] == "Sleep":
        print("  Rest; no scheduled-work catch-up while sleeping.")
    else:
        print("  " + priority(c, now, agenda))
    if error:
        print(f"\nCalendar: {error}")
    print("\nNote: Suggestions do NOT modify events in Google Calendar.")


def local_extract(text: str):
    """Conservative extraction without LLM: keep raw report but do not invent durations."""
    return []


def ollama_extract(text: str):
    schema = {"type": "object", "properties": {"entries": {"type": "array", "items": {
        "type": "object", "properties": {"project": {"type": "string"},
        "minutes": {"type": ["integer", "null"]}, "detail": {"type": "string"}},
        "required": ["project", "minutes", "detail"], "additionalProperties": False}}},
        "required": ["entries"], "additionalProperties": False}
    prompt = ("Extract completed activities from this spoken activity report. Output JSON only. "
              "Use one item per explicit activity. Project should be one of Docs, Game development, DI, DJ, VI, DS, "
              "Drawing, Cardio, Weightlifting, Sleep or a concise project name. "
              "Convert explicit durations to minutes; if uncertain or not stated use null. "
              "Never guess a duration or turn future plans into completed actions. "
              "Preserve an honest summary for each item.\n\nREPORT:\n" + text)
    payload = {"model": MODEL, "stream": False, "format": schema, "messages": [{"role": "user", "content": prompt}], "options": {"temperature": 0}}
    data = json.dumps(payload).encode()
    request = urllib.request.Request(os.getenv("NOW_OLLAMA_URL", "http://127.0.0.1:11434/api/chat"), data=data, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=120) as reply:
            result = json.load(reply)
        parsed = json.loads(result["message"]["content"])
        clean = []
        for item in parsed.get("entries", []):
            name = canonical(str(item.get("project", "")).strip())
            minutes = item.get("minutes")
            if not name or (minutes is not None and (not isinstance(minutes, int) or minutes < 0 or minutes > 1440)):
                continue
            clean.append((name, minutes, str(item.get("detail", ""))[:1000]))
        return clean, None
    except (urllib.error.URLError, ValueError, KeyError, TimeoutError) as e:
        return [], str(e)


def add_log(c, content: str, source: str = "text", use_llm=True):
    if not content.strip():
        raise ValueError("Activity report is empty")
    now = current_time()
    cur = c.execute("INSERT INTO journal(created, raw, source) VALUES (?,?,?)", (stamp(now), content.strip(), source))
    entry_id = cur.lastrowid
    c.commit()  # Preserve the report even if the local model crashes or is cancelled.
    items, err = ollama_extract(content) if use_llm else (local_extract(content), None)
    for project, minutes, detail in items:
        c.execute("INSERT INTO journal_items(journal_id,project,minutes,detail) VALUES (?,?,?,?)", (entry_id, project, minutes, detail))
    c.commit()
    print(f"Saved local journal entry #{entry_id}.")
    if items:
        print("Extracted (reported estimates, NOT active-timer measurements):")
        for project, minutes, detail in items:
            print(f"  {project} — {str(minutes) + ' min' if minutes is not None else 'duration unknown'} — {detail}")
    elif err:
        print("Saved raw text; could not reach the local Ollama model. No details invented.")
        print(f"  {err[:180]}")
    else:
        print("Saved raw text. Transcription and structured extraction are separate optional steps.")


def record_voice(c):
    pw = shutil.which("pw-record")
    whisper = os.getenv("WHISPER_CLI") or shutil.which("whisper-cli")
    model = os.getenv("WHISPER_MODEL")
    if not pw:
        print("Missing `pw-record` (PipeWire tools).")
        return 2
    if not whisper or not model or not Path(model).is_file():
        print("Set WHISPER_CLI (if needed) and WHISPER_MODEL to the local whisper.cpp model file.")
        return 2
    with tempfile.TemporaryDirectory(prefix="now-voice-") as dir_name:
        p = Path(dir_name)
        recording = p / "report.wav"
        out = p / "transcript"
        command = [pw, "--rate=16000", "--channels=1", "--format=s16", "--sample-count=1920000", str(recording)]
        print("Recording locally. Press Enter to finish, or it stops after 2 minutes.", flush=True)
        proc = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            # Monitor both Enter and natural 2-minute recording completion.
            while proc.poll() is None:
                readable, _, _ = select.select([sys.stdin], [], [], 0.25) if sys.stdin.isatty() else ([], [], [])
                if readable:
                    sys.stdin.readline()
                    proc.send_signal(signal.SIGINT)
                    break
            _, err = proc.communicate(timeout=10)
        except (KeyboardInterrupt, subprocess.TimeoutExpired):
            proc.terminate()
            proc.communicate()
            err = b""
        if not recording.exists() or recording.stat().st_size < 1000:
            print("Recording failed: " + err.decode(errors="replace")[:300])
            return 2
        print("Transcribing on this machine...")
        result = subprocess.run([whisper, "-m", model, "-f", str(recording), "-otxt", "-of", str(out)], capture_output=True, text=True)
        txt = out.with_suffix(".txt")
        if result.returncode != 0 or not txt.exists():
            print("Transcription failed: " + result.stderr[-500:])
            return 2
        words = txt.read_text(encoding="utf8").strip()
        print("\nTRANSCRIPT\n" + words + "\n")
        add_log(c, words, source="voice")
    return 0



def bar(c):
    """One-line JSON for a Waybar custom module, refreshed on demand."""
    now = current_time()
    active = active_session(c)
    agenda, err = cached_agenda(c, now, days=14)
    if active:
        suggestion = f"{active['project']} • {duration_minutes(active['started'], now)}m"
    else:
        suggestion = priority(c, now, agenda)
    nearest = next((e for e in agenda if fixed_event(e) and e['when'] >= now), None)
    text = suggestion.split(':', 1)[0][:32]
    tooltip = f"{now:%A %H:%M}\n{suggestion}"
    if nearest:
        tooltip += f"\nNext fixed: {nearest['when']:%a %H:%M} — {nearest['title']}"
    if err:
        tooltip += "\nGoogle Calendar not connected"
    print(json.dumps({"text": text, "tooltip": tooltip, "class": "sleep" if active and active['project'] == 'Sleep' else 'work'}, ensure_ascii=False))


def notify(c):
    """Send at most one notification per changed recommendation each day."""
    if not shutil.which('notify-send'):
        print('`notify-send` not installed; no desktop notification.')
        return
    ongoing = active_session(c)
    if ongoing and ongoing['project'] == 'Sleep':
        return
    now = current_time()
    agenda, _ = cached_agenda(c, now, days=14)
    action = priority(c, now, agenda)
    fixed = next((e for e in agenda if fixed_event(e) and e['when'] >= now), None)
    subtitle = action
    if fixed and fixed['when'] <= now + timedelta(hours=1):
        subtitle = f"Next fixed event at {fixed['when']:%H:%M}: {fixed['title']}\n{action}"
    key = hashlib.sha256((str(now.date()) + subtitle).encode()).hexdigest()
    prior = c.execute("SELECT value FROM settings WHERE key = 'last_notification'").fetchone()
    if prior and prior['value'] == key:
        return
    result = subprocess.run(['notify-send', 'Now — your next action', subtitle], capture_output=True, text=True)
    if result.returncode == 0:
        c.execute("INSERT OR REPLACE INTO settings(key,value) VALUES ('last_notification',?)", (key,))
        c.commit()


def show_report(c):
    stats = recent_totals(c)
    print("\nTRACKED SESSION TIME (this week)")
    for p, v in sorted(stats["week"].items(), key=lambda x: -x[1]):
        print(f"  {p:22} {v / 60:5.2f} h")
    print("\nREPORTED DURATIONS (rough estimates; not added to timed sessions)")
    for p, v in sorted(stats["reported_week"].items(), key=lambda x: -x[1]):
        print(f"  {p:22} {v / 60:5.2f} h")
    print("\nRECENT JOURNAL ENTRIES")
    for j in c.execute("SELECT * FROM journal ORDER BY id DESC LIMIT 5"):
        print(f"  {dt(j['created']):%a %H:%M}  {j['raw'][:125]}")


def export_json(c, out_path):
    result = {}
    for table in ("sessions", "journal", "journal_items", "deadlines"):
        result[table] = [dict(r) for r in c.execute(f"SELECT * FROM {table}")]
    Path(out_path).write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(f"Exported journal and sessions: {out_path}")


def main(argv=None):
    parser = argparse.ArgumentParser(description="Adaptive local journal + next-action assistant (MIT)")
    p = parser.add_subparsers(dest="command")
    p.add_parser("now", help="What am I doing and what should I do next?")
    p.add_parser("stop", help="Stop currently tracked work")
    p.add_parser("wake", help="Finish sleep session")
    p.add_parser("sleep", help="Start tracking sleep (manual)")
    p.add_parser("voice", help="Record up to 2 minutes and transcribe offline")
    p.add_parser("report", help="This week's tracked time and voice reports")
    p.add_parser("agenda", help="Print live Google Calendar agenda if connected")
    p.add_parser("bar", help="Emit a Waybar JSON status line")
    p.add_parser("snapshot", help="Emit a structured read-only JSON state for Omarchy Quickshell")
    p.add_parser("notify", help="Send a deduplicated local desktop notification")
    start = p.add_parser("start", help="Start tracking a project")
    start.add_argument("project", nargs="+", help="e.g. Docs, Game development, DI")
    log = p.add_parser("log", help="Journal what you did; local Ollama parsing optional")
    log.add_argument("report", nargs="+", help="Free-form retrospective report")
    log.add_argument("--raw", action="store_true", help="Save without LLM extraction")
    ddl = p.add_parser("deadline", help="Manage fixed deadlines stored locally")
    ddl_sub = ddl.add_subparsers(dest="action", required=True)
    ddl_sub.add_parser("list")
    add = ddl_sub.add_parser("add")
    add.add_argument("title")
    add.add_argument("when", help="Local YYYY-MM-DDTHH:MM")
    done = ddl_sub.add_parser("done")
    done.add_argument("id", type=int)
    exporter = p.add_parser("export", help="Export local database as JSON")
    exporter.add_argument("file")
    args = parser.parse_args(argv)
    with db() as c:
        if args.command in (None, "now"):
            show_now(c)
        elif args.command == "start":
            record_start(c, " ".join(args.project))
        elif args.command == "stop":
            record_stop(c)
        elif args.command == "sleep":
            record_start(c, "Sleep")
        elif args.command == "wake":
            record_stop(c, expected="Sleep")
            show_now(c)
        elif args.command == "voice":
            return record_voice(c)
        elif args.command == "log":
            add_log(c, " ".join(args.report), use_llm=not args.raw)
        elif args.command == "bar":
            bar(c)
        elif args.command == "snapshot":
            print(json.dumps(snapshot(c), ensure_ascii=False))
        elif args.command == "notify":
            notify(c)
        elif args.command == "report":
            show_report(c)
        elif args.command == "agenda":
            ev, error = google_agenda()
            for e in ev:
                print(f"{e['when']:%a %d %b %H:%M}  {e['title']}")
            if error:
                print(error)
        elif args.command == "deadline":
            if args.action == "add":
                add_deadline(c, args.title, args.when)
            elif args.action == "list":
                for d in deadlines(c, include_done=True):
                    print(f"{d['id']:>3}  {'✓' if d['done'] else ' '}  {dt(d['due']):%a %d %b %H:%M}  {d['title']}")
            elif args.action == "done":
                cur = c.execute("UPDATE deadlines SET done=1 WHERE id=?", (args.id,))
                c.commit()
                print("Marked done." if cur.rowcount else "No such deadline ID.")
        elif args.command == "export":
            export_json(c, args.file)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, sqlite3.Error) as exc:
        print(f"Error: {exc}", file=sys.stderr)
        sys.exit(2)

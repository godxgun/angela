# Angela

**Angela is a small local-first assistant for irregular working hours.** MIT-licensed Python/SQLite CLI with a Quickshell bar widget for Omarchy Quattro, Handy speech-to-text input, and optional local Ollama parsing.

- **Calendar**: read-only scheduled commitments and deadlines via `gcalcli` (classes and exams remain fixed).
- **Scheduler**: deterministic on-the-spot recommendation, recalculated from the actual clock, deadlines, tracked progress, and sleep. Does not try to schedule an imaginary fixed wake-up time.
- **Activity log**: timed sessions plus dictated retrospective reports. Ollama converts reports into *estimated* task durations; it does not get authority to reschedule your Google Calendar or execute commands.
- **Omarchy**: a bar widget and dropdown panel compatible with the current Quattro plugin system. The plugin reads `now snapshot` every minute; Google Calendar responses are cached for 10 minutes.

It is a **prototype**, not an autonomous planner. It does not detect sleep on its own, update Google Calendar events, or automatically discover work from the active app. ActivityWatch support and safe, opt-in Calendar writeback are future work.

## Install on Omarchy / Arch Linux

Python 3.10+ is the only requirement for the core CLI. No pip dependencies.

```bash
./install.sh --omarchy
now snapshot
```

The installer copies the CLI into `~/.local/bin/now`, the plugin files to `~/.config/omarchy/plugins/io.github.godxgun.angela/`, and two *disabled-by-default* optional notification systemd user units. On Omarchy it validates and enables the bar widget using the installed `omarchy plugin` CLI.

Make sure `~/.local/bin` is on the PATH visible to `omarchy-shell` (`/usr/bin/env now snapshot` must work); if the bar says `Now` and the panel fails to update, first check `now snapshot` in a terminal. Some shell environments need a logout/login after PATH changes.

The correct way to integrate with Omarchy Quattro is **one shell process**, not a separate Quickshell process. The plugin is a `bar-widget` with `manifest.json`, `BarWidget.qml` and `Panel.qml` at the repository root, so it can also be installed with `omarchy plugin add <repo-url> --enable` after publishing. [Omarchy plugin reference](https://omarchy.org/manual/shell-plugins/).

Check it on the real desktop:

```bash
omarchy plugin list
omarchy plugin validate ~/.config/omarchy/plugins/io.github.godxgun.angela
omarchy-shell shell summon io.github.godxgun.angela '{}'
# Optional; availability depends on the installed Qt tooling:
qmllint -I "$OMARCHY_PATH/shell" ~/.config/omarchy/plugins/io.github.godxgun.angela/{BarWidget,Panel}.qml
```

**Note**: Quickshell and Omarchy are not available in the automated test environment used to prepare this repository. The plugin follows Omarchy's published Quattro contract but must be run-tested in your session. Check for Quattro support first; older Omarchy releases using Waybar can use `now bar` instead.

## Handy + Ollama: two-minute spoken check-ins

[Handy](https://handy.computer/) is **speech-to-text (dictation)**, not text-to-speech. There is no need to install a second speech recognizer for this workflow.

1. Click **Now** in the Omarchy bar to open the panel.
2. Click the activity report field; use your normal Handy shortcut, or the panel's **Handy dictation** button to toggle transcription (requires `handy` in PATH).
3. Say e.g. *"I was working on the Docs editor for two hours, then practiced drawing for 45 minutes."*
4. When Handy has inserted the text, press **Save report (Ollama)**. To avoid the model, press **Save raw**.

Handy inserts into the **currently focused text field**, so don't start dictating while the focus is elsewhere. It works with the supported Handy CLI `handy --toggle-transcription`, which controls an already-running instance. [Handy CLI documentation](https://handy.computer/docs/cli). On Wayland, test Handy's clipboard/paste method inside the panel; using the existing Handy keyboard shortcut is also fine.

Install [Ollama](https://ollama.com/) and download a small model:

```bash
ollama pull qwen2.5:1.5b
ollama serve  # only if Ollama isn't running already
now log 'I spent two hours on docs and another hour on DS.'
```

Ollama is optional. The default endpoint is loopback `http://127.0.0.1:11434/api/chat` (override with `NOW_OLLAMA_URL`) and default model is `qwen2.5:1.5b` (override with `NOW_MODEL`). The app submits **only the dictated report** to Ollama for structured activity extraction. No raw calendar credentials or entire calendar history go into that prompt. Local inference may be CPU-bound on low-VRAM GPUs.

The `now voice` command from v0.1 still supports whisper.cpp and PipeWire as a standalone optional fallback, but **Handy in the panel is the recommended method** for Omarchy. The older CLI voice path requires `pw-record`, `WHISPER_CLI` and `WHISPER_MODEL`.

## CLI

```bash
now                    # Human-readable plan
now snapshot           # Structured JSON for the Quickshell widget
now start Docs
now start 'Game development'
now stop
now sleep
now wake
now log 'I worked on VI for an hour, then made sketches for 30 minutes'
now log --raw 'Journal entry, never processed by a model'
now report
now deadline add 'University submission' 2026-10-19T14:00
now deadline list
now deadline done 1
now agenda             # gcalcli live read (if configured)
now bar                # Legacy Waybar JSON output
now notify             # Optional local desktop notification
now export backup.json
```

`now snapshot` is a read-only JSON contract and includes current tracking, next-action recommendation, nearest deadline, next fixed event, and today's minutes. Its calendar adapter is cached for ten minutes and marks stale data as stale if sync fails. The recommendations never silently alter your Google Calendar.

### Deterministic scheduling policy (current MVP)

1. If sleep tracking is active, don't suggest work. If a fixed event is underway or begins within twenty minutes, give it precedence.
2. If an unfinished deadline is within 72 hours, prioritize preparing for it. No LLM is required to make this decision.
3. Aim for one hour each per day of DI, DJ, VI, and DS; strength training Monday/Wednesday/Friday and cardio on other days; two hours of drawing.
4. Divide the **remaining** work between Docs and Game Development according to actual logged weekly time, aiming for 50/50.

This is a queue, **not a hard commitment to working 07:00–23:00**. Missing yesterday's preferred block does not move your university class. Every timer start, stop, and wake refreshes the plan. The app does not treat inactivity as proven sleep and does not automatically fill all available time (rest and meals remain yours to decide).

### Google Calendar (optional, read-only)

Install [`gcalcli`](https://github.com/insanum/gcalcli) with e.g. `pipx install gcalcli`, authenticate it using your **own** Google Calendar OAuth credentials, then verify `gcalcli agenda` and `now agenda`. The ChatGPT Google Calendar connector cannot supply local app credentials. `gcalcli` agenda TSV is parsed locally, including the `Hora limite: HH:MM` phrase on a Portuguese university all-day submission entry if the event description is included.

While Calendar isn't connected, the app still works with manually entered `now deadline add` records. Note that **calendar events do not automatically become locally completable deadlines**: mark a local deadline done with `now deadline done ID`, and verify remote due events in Google Calendar. The prototype doesn't attempt to infer from "Entregue? Não" when an assignment was subsequently submitted.

### Optional notifications

```bash
systemctl --user enable --now now-notify.timer
# Revert:
systemctl --user disable --now now-notify.timer
```

Notifications run hourly, deduplicate repeated recommendations, and are suppressed while a tracked sleep session is active. The Quickshell bar itself polls local JSON every minute; it does not require this timer.

## Privacy, data, and limits

All data is in SQLite at `$XDG_DATA_HOME/now-local/now.sqlite3` or `~/.local/share/now-local/now.sqlite3`. Override with `NOW_DB`. Set `NOW_TZ` for your timezone (default `Europe/Lisbon`). The repo ignores databases, model weights, audio and `.env` files. No microphone is always on; Handy receives audio only when activated. The optional direct `now voice` path deletes temporary WAV/transcription files and retains the transcript in the journal.

Retrospective reports with times (e.g. "I coded for 2 hours") are kept as **reported estimates** separate from timer measurements; the scheduler takes the maximum rather than summing them to reduce double-counting. It is not reliable enough to treat all reports as audited timesheets. Ollama extraction may make mistakes; verify critical records.

The Omarchy plugin executes `now` as your normal desktop user. Omarchy plugin QML is **unsandboxed**; review any plugin before enabling it.

## Develop

```bash
python -m unittest discover -s tests -v
python -m py_compile now.py
```

No network calls are needed for the tests. The core is standard-library Python, and the QML depends on Omarchy's own shell imports. Tested: 12 Python tests; **live Omarchy/Handy integration not yet tested**.

## Repository and contributing

Project: [godxgun/angela](https://github.com/godxgun/angela). The program is called `now`, so you can ask for your next task without typing the full project name.

The Omarchy module ID is `io.github.godxgun.angela`. If you installed the previous experimental module under `io.github.valvesky.now-local`, disable that old plugin before installing Angela to avoid duplicate bar widgets.

This repository is an MIT-licensed prototype. Test its Quickshell integration on your Omarchy desktop before relying on it for deadline notifications.

## License

MIT. See [LICENSE](LICENSE).

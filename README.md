# Angela

A small, MIT-licensed **Rust + egui desktop assistant** for irregular working hours.
Type or speak a check-in, see your week, track what actually happened, and decide
what to do next. Omaspeak handles local speech recognition; Ollama interprets
reports and helps order flexible work. No web frontend or Python runtime is
required for the desktop app.

## Run on Omarchy / Arch

```sh
sudo pacman -S rustup base-devel pkgconf pipewire libxkbcommon wayland mesa
rustup default stable
cargo run --locked --release
# Or install the binary and desktop launcher:
./install-rust.sh
```

Install your [Omaspeak fork](https://github.com/valvesky/omaspeak) separately.
In **Settings & connections**, set its executable path (the executable may still
be named `handy`). Use **Open Omaspeak / download models** for its existing voice
model manager, then **Find voice models** and select an installed model.
Angela uses the fork's actual `--list-models --json` and
`--transcribe-file WAV --model ID --json` interfaces. Voice model downloads and
microphone/accelerator configuration remain owned by Omaspeak.

Install/start Ollama, then use **List models** or **Download model** in Settings.
The default is `qwen2.5:1.5b`, suitable for trying CPU inference on a small laptop.
The model is configurable. Ollama calls are restricted to loopback addresses.

## Google Calendar connection

The old Python app depended on an independently configured `gcalcli` account.
The desktop app connects directly to the Google Calendar API:

1. In [Google Cloud Console](https://console.cloud.google.com/), enable the Google
   Calendar API. Configure the OAuth consent screen and add your own account as a
   test user if the app is in testing mode.
2. Create an OAuth client with application type **Desktop app**, download its JSON,
   and put its full path in Angela's **Google Calendar** settings.
3. Click **Connect Google** and complete consent in your browser. The app listens
   on a temporary loopback port, verifies OAuth state and uses PKCE.

Access tokens refresh automatically. Sync fetches all calendar pages and expanded
recurring occurrences for the displayed week, every five minutes and on demand.
Calendar checkboxes control visibility and which commitments block the local plan.
University entries titled Entrega, Deadline or Submission become local tasks;
Portuguese `Hora limite: HH:MM` descriptions supply their due time. Completing
these tasks affects Angela only, and subsequent sync preserves that completion.
Errors are shown in the app; previously cached events remain visible with their
last successful sync time. Disconnect removes the local token and calendar cache;
you can also revoke access in your Google account.

Google access is **read-only**, as agreed in the original spec. Tracked activity,
requirements and flexible intentions update locally; Angela never rewrites your
classes or appointments. The ChatGPT Calendar connector cannot supply a desktop
app's OAuth credentials. Live sign-in must be completed on your own machine.

## Everyday use

- **Speak → Stop & transcribe** records a temporary 16 kHz mono WAV with PipeWire,
  then asks Omaspeak to transcribe it. The result lands in the same prompt as typed
  text. Audio is removed after transcription or on application exit.
- **Interpret → Apply** previews structured changes from your local Ollama model.
  Try “I worked on Docs for an hour”, “I'm going to sleep”, “I woke up”,
  “I need 180 minutes of cardio each week”, or “Add VI submission, two hours,
  due Monday at 23:59”. You can edit a transcript before interpretation.
- **Save note** keeps the text locally without asking a model to interpret it.
- **Start next**, **Start** beside a task, **Stop activity**, **Sleep** and
  **I'm awake** work without Ollama. Sleep has no assumed bedtime or wake time.
- **Requirements → Edit** changes minimum minutes, daily/weekly periods and days.
  Progress uses actual logged intervals. Overlapping reports of the same activity
  are counted once, and sessions crossing midnight are clipped to local dates.
- The **flexible queue** has completion checkboxes and up/down controls. Deadlines
  within 72 hours take precedence. Targets start with DI/DJ/VI/DS at one hour each,
  drawing at two hours, strength M/W/F and cardio on other days at one hour.
  Remaining suggestions balance actual weekly Docs/game-development minutes.
- **Fill gaps** makes local, proposed blocks for an explicit 1–24 hour horizon.
  **Plan with Ollama** supplies remaining requirements, tasks, fixed commitments
  and recent sleep to the model, previews its ordering, then uses the same
  deterministic placement code. Both preserve fixed events and stop proposing
  work while sleep tracking is active. Nothing assumes every gap must be work.

In the calendar, blue means a fixed commitment, green means tracked activity,
and purple means a proposed block. Hover for full titles and times. Tasks with
estimated effort still need to be checked off when finished; a timer alone does
not prove that the task is complete.

## Data and development

App state is local JSON at `$XDG_DATA_HOME/angela/state.json` (normally
`~/.local/share/angela/state.json` on Linux). `ANGELA_DATA_DIR` overrides the data
folder. Writes are atomic; directories and files use private permissions on Unix.
Google tokens are stored separately in `google-token.json`. Don't commit or share
that folder. The model receives scheduling context, never OAuth tokens. Save notes
and interpreted reports remain in the journal inside `state.json`.

The earlier Python/Quickshell prototype remains available and its documentation
is in [docs/legacy-python.md](docs/legacy-python.md). Its SQLite history and the new
desktop state are separate; this version does not automatically migrate that history.

The desktop implementation has five modules: UI, domain/scheduling, Google OAuth
and Calendar, local services, and persistence. Network/inference/transcription
work runs off the UI thread.

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
python -m unittest discover -s tests -v  # legacy prototype
```

16 automated tests cover scheduler invariants, Calendar pagination and deadlines,
Ollama requests, model-output validation, and headless UI rendering. The legacy
Python suite also passes (12 tests). Release build, formatting, and Clippy checks pass.
Live
Google consent, your Omaspeak build, microphone capture, and real Ollama inference
need checking on the target Omarchy desktop.

mod domain;
mod google;
mod services;
mod storage;
use chrono::{Datelike, Duration, NaiveDate, Timelike, Utc};
use domain::{Action, Requirement, State, Task};
use eframe::egui::{self, Color32, RichText};
use std::{
    process::Child,
    sync::mpsc::{self, Receiver},
    time::Duration as Wait,
};

type CalendarBlock = (
    String,
    chrono::DateTime<Utc>,
    chrono::DateTime<Utc>,
    bool,
    Color32,
);

enum Reply {
    Synced(Vec<domain::Calendar>, Vec<domain::Event>),
    Actions(Vec<Action>),
    Transcript(String),
    VoiceModels(serde_json::Value),
    Models(Vec<String>),
    Connected,
    Message(String),
}
struct App {
    state: State,
    prompt: String,
    status: String,
    settings: bool,
    requirements: bool,
    week: NaiveDate,
    pending: Option<Vec<Action>>,
    pending_report: String,
    receiver: Option<Receiver<anyhow::Result<Reply>>>,
    recorder: Option<Child>,
    voice_models: serde_json::Value,
    models: Vec<String>,
    new_title: String,
    new_minutes: i64,
    plan_hours: i64,
    planning: bool,
    dirty: bool,
    last_tick: std::time::Instant,
    initial_scroll: Option<f32>,
}
impl App {
    fn new(ctx: &egui::Context, state: State) -> Self {
        ctx.set_visuals(egui::Visuals::dark());
        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(10., 10.);
        style.spacing.button_padding = egui::vec2(12., 7.);
        style.visuals.panel_fill = Color32::from_rgb(20, 22, 27);
        style.visuals.window_fill = Color32::from_rgb(25, 28, 34);
        style.visuals.selection.bg_fill = Color32::from_rgb(52, 88, 91);
        ctx.set_style(style);
        let today = Utc::now().with_timezone(&state.timezone).date_naive();
        let initial_scroll = Some(
            Utc::now()
                .with_timezone(&state.timezone)
                .hour()
                .saturating_sub(2) as f32
                * 42.,
        );
        Self {
            state,
            prompt: String::new(),
            status: "Ready · local data".into(),
            settings: false,
            requirements: false,
            week: today - Duration::days(today.weekday().num_days_from_monday() as i64),
            pending: None,
            pending_report: String::new(),
            receiver: None,
            recorder: None,
            voice_models: serde_json::Value::Null,
            models: vec![],
            new_title: String::new(),
            new_minutes: 60,
            plan_hours: 8,
            planning: false,
            dirty: false,
            last_tick: std::time::Instant::now() - Wait::from_secs(300),
            initial_scroll,
        }
    }
    fn save(&mut self) {
        match storage::save(&self.state) {
            Ok(()) => self.dirty = false,
            Err(e) => self.status = format!("Save failed: {e:#}"),
        }
    }
    fn job(&mut self, label: &str, f: impl FnOnce() -> anyhow::Result<Reply> + Send + 'static) {
        if self.receiver.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.status = label.into();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
    }
    fn sync(&mut self) {
        let s = self.state.clone();
        let week = s.midnight(self.week);
        self.job("Syncing Google Calendar…", move || {
            let (c, e) = google::sync(&s, week)?;
            Ok(Reply::Synced(c, e))
        });
    }
    fn poll(&mut self) {
        let reply = self.receiver.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(v) => Some(v),
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(anyhow::anyhow!(
                "Background operation ended unexpectedly"
            ))),
            Err(_) => None,
        });
        if let Some(reply) = reply {
            self.receiver = None;
            match reply {
                Err(e) => self.status = format!("{e:#}"),
                Ok(r) => {
                    self.status = "Ready".into();
                    match r {
                        Reply::Synced(c, e) => {
                            let from = self.state.midnight(self.week) - Duration::days(1);
                            self.state.replace_calendar(
                                c,
                                e,
                                from,
                                from + Duration::days(9),
                                Utc::now(),
                            );
                            self.save();
                            self.status = "Calendar up to date".into();
                        }
                        Reply::Actions(a) => self.pending = Some(a),
                        Reply::Transcript(t) => {
                            self.prompt = t;
                            self.status = "Transcript ready · edit, then Interpret".into();
                        }
                        Reply::VoiceModels(m) => self.voice_models = m,
                        Reply::Models(m) => self.models = m,
                        Reply::Connected => self.sync(),
                        Reply::Message(m) => self.status = m,
                    }
                }
            }
        }
    }
    fn side(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sidebar")
            .exact_width(250.)
            .show(ctx, |ui| {
                ui.add_space(12.);
                ui.heading("Angela");
                ui.label(RichText::new("Your time, as it happens.").weak());
                ui.add_space(20.);
                ui.label(RichText::new("NEXT").small().weak());
                ui.label(RichText::new(self.state.next(Utc::now())).size(19.));
                if let Some(s) = self.state.active().cloned() {
                    ui.add_space(8.);
                    ui.label(format!(
                        "Tracking {} · {} min",
                        s.title,
                        (Utc::now() - s.start).num_minutes()
                    ));
                    if ui
                        .button(if s.title == "Sleep" {
                            "I'm awake"
                        } else {
                            "Stop activity"
                        })
                        .clicked()
                    {
                        self.state.stop(Utc::now());
                        self.state.plan.clear();
                        self.save();
                    }
                } else {
                    ui.horizontal(|ui| {
                        if ui.button("Start next").clicked() {
                            if let Some((title, _)) = self.state.queue(Utc::now()).first() {
                                self.state.start(title.clone(), Utc::now());
                                self.save();
                            }
                        }
                        if ui.button("Sleep").clicked() {
                            self.state.start("Sleep".into(), Utc::now());
                            self.save();
                        }
                    });
                }
                ui.add_space(20.);
                ui.horizontal(|ui| {
                    ui.strong("Requirements");
                    if ui.small_button("Edit").clicked() {
                        self.requirements = true;
                    }
                });
                let now = Utc::now();
                for r in &self.state.requirements {
                    if !self.state.applicable(r, now) {
                        continue;
                    }
                    let left = self.state.remaining(r, now);
                    let done = r.minutes - left;
                    ui.label(if left == 0 {
                        RichText::new(&r.title).strikethrough().weak()
                    } else {
                        RichText::new(&r.title)
                    });
                    ui.add(
                        egui::ProgressBar::new(done as f32 / r.minutes.max(1) as f32).text(
                            format!(
                                "{done}/{} min{}",
                                r.minutes,
                                if r.weekly { " this week" } else { " today" }
                            ),
                        ),
                    );
                }
                ui.add_space(16.);
                ui.strong("Calendars");
                if self.state.calendars.is_empty() {
                    ui.label(RichText::new("Connect in Settings").weak());
                }
                for c in &mut self.state.calendars {
                    if ui.checkbox(&mut c.visible, &c.title).changed() {
                        self.dirty = true;
                    }
                }
                ui.add_space(15.);
                if ui.button("Settings & connections").clicked() {
                    self.settings = true;
                }
            });
    }
    fn calendar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.receiver.is_none(), egui::Button::new("‹"))
                .clicked()
            {
                self.week -= Duration::days(7);
                self.sync();
            }
            if ui
                .add_enabled(self.receiver.is_none(), egui::Button::new("Today"))
                .clicked()
            {
                let d = Utc::now().with_timezone(&self.state.timezone).date_naive();
                self.week = d - Duration::days(d.weekday().num_days_from_monday() as i64);
                self.sync();
            }
            if ui
                .add_enabled(self.receiver.is_none(), egui::Button::new("›"))
                .clicked()
            {
                self.week += Duration::days(7);
                self.sync();
            }
            ui.heading(format!(
                "{} — {}",
                self.week.format("%d %b"),
                (self.week + Duration::days(6)).format("%d %b %Y")
            ));
            if ui
                .add_enabled(self.receiver.is_none(), egui::Button::new("Sync"))
                .clicked()
            {
                self.sync();
            }
        });
        ui.horizontal(|ui| {
            ui.label("Plan the next");
            ui.add(
                egui::DragValue::new(&mut self.plan_hours)
                    .range(1..=24)
                    .suffix(" hours"),
            );
            if ui.button("Fill gaps").clicked() {
                self.state.fill(Utc::now(), self.plan_hours);
                self.save();
            }
            if ui
                .add_enabled(
                    self.receiver.is_none(),
                    egui::Button::new("Plan with Ollama"),
                )
                .clicked()
            {
                let s = self.state.clone();
                self.planning = true;
                self.pending_report = "Requested local Ollama plan".into();
                self.job("Planning from requirements and commitments…", move || {
                    Ok(Reply::Actions(services::plan(&s)?))
                });
            }
            if ui.small_button("Clear intentions").clicked() {
                self.state.plan.clear();
                self.save();
            }
            ui.label(RichText::new("Fixed · tracked · proposed").weak().small());
        });
        let mut scroll = egui::ScrollArea::both().id_salt("week").max_height(440.);
        if let Some(offset) = self.initial_scroll.take() {
            scroll = scroll.vertical_scroll_offset(offset);
        }
        scroll.show(ui, |ui| {
            let width = ui.available_width().max(770.);
            let hour = 42.;
            let gutter = 44.;
            let day = (width - gutter) / 7.;
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(width, 24. * hour + 48.), egui::Sense::hover());
            let p = ui.painter();
            let font = egui::FontId::proportional(12.);
            for d in 0..7 {
                let date = self.week + Duration::days(d);
                let x = rect.left() + gutter + d as f32 * day;
                p.text(
                    egui::pos2(x + 6., rect.top() + 10.),
                    egui::Align2::LEFT_TOP,
                    date.format("%a %d").to_string(),
                    font.clone(),
                    Color32::LIGHT_GRAY,
                );
                p.line_segment(
                    [
                        egui::pos2(x, rect.top() + 44.),
                        egui::pos2(x, rect.bottom()),
                    ],
                    egui::Stroke::new(1_f32, Color32::from_gray(42)),
                );
            }
            for h in 0..=24 {
                let y = rect.top() + 48. + h as f32 * hour;
                p.text(
                    egui::pos2(rect.left(), y),
                    egui::Align2::LEFT_CENTER,
                    format!("{h:02}"),
                    font.clone(),
                    Color32::GRAY,
                );
                p.line_segment(
                    [
                        egui::pos2(rect.left() + gutter, y),
                        egui::pos2(rect.right(), y),
                    ],
                    egui::Stroke::new(1_f32, Color32::from_gray(35)),
                );
            }
            let now = Utc::now();
            let mut items: Vec<CalendarBlock> = self
                .state
                .visible_events()
                .into_iter()
                .map(|e| {
                    (
                        e.title.clone(),
                        e.start,
                        e.end,
                        e.all_day,
                        Color32::from_rgb(54, 70, 91),
                    )
                })
                .collect();
            items.extend(self.state.sessions.iter().map(|s| {
                (
                    s.title.clone(),
                    s.start,
                    s.end.unwrap_or(now),
                    false,
                    Color32::from_rgb(40, 91, 79),
                )
            }));
            items.extend(self.state.plan.iter().map(|e| {
                (
                    e.title.clone(),
                    e.start,
                    e.end,
                    false,
                    Color32::from_rgb(83, 65, 99),
                )
            }));
            for d in 0..7 {
                let date = self.week + Duration::days(d);
                let a = self.state.midnight(date);
                let b = self.state.midnight(date + Duration::days(1));
                let x = rect.left() + gutter + d as f32 * day;
                let mut all = 0;
                let mut lane_ends: Vec<chrono::DateTime<Utc>> = vec![];
                let mut visible: Vec<_> = items
                    .iter()
                    .filter(|(_, start, end, _, _)| *start < b && *end > a)
                    .collect();
                visible.sort_by_key(|(_, start, _, _, _)| *start);
                for (title, start, end, all_day, color) in visible {
                    let r = if *all_day {
                        all += 1;
                        egui::Rect::from_min_size(
                            egui::pos2(x + 3., rect.top() + 26.),
                            egui::vec2(day - 6., 17.),
                        )
                    } else {
                        let lane = lane_ends
                            .iter()
                            .position(|e| *e <= *start)
                            .unwrap_or(lane_ends.len());
                        if lane == lane_ends.len() {
                            lane_ends.push(*end);
                        } else {
                            lane_ends[lane] = *end;
                        }
                        let y = rect.top()
                            + 48.
                            + (start
                                .max(&a)
                                .with_timezone(&self.state.timezone)
                                .time()
                                .signed_duration_since(chrono::NaiveTime::MIN)
                                .num_seconds() as f32
                                / 3600.)
                                * hour;
                        let duration = (*end.min(&b) - *start.max(&a)).num_seconds() as f32 / 3600.;
                        egui::Rect::from_min_size(
                            egui::pos2(x + 3. + (lane % 3) as f32 * 9., y),
                            egui::vec2(
                                day - 9. - (lane % 3) as f32 * 9.,
                                (duration * hour).max(16.),
                            ),
                        )
                    };
                    p.rect_filled(r, 3., *color);
                    let cp = p.with_clip_rect(r.shrink(3.));
                    cp.text(
                        r.min + egui::vec2(4., 3.),
                        egui::Align2::LEFT_TOP,
                        title,
                        font.clone(),
                        Color32::WHITE,
                    );
                    let response =
                        ui.interact(r, ui.id().with((d, title, start)), egui::Sense::hover());
                    response.on_hover_text(format!(
                        "{title}\n{} – {}",
                        start.with_timezone(&self.state.timezone).format("%a %H:%M"),
                        end.with_timezone(&self.state.timezone).format("%a %H:%M")
                    ));
                }
                if all > 1 {
                    p.text(
                        egui::pos2(x + day - 4., rect.top() + 26.),
                        egui::Align2::RIGHT_TOP,
                        format!("+{}", all - 1),
                        font.clone(),
                        Color32::WHITE,
                    );
                }
                if now >= a && now < b {
                    let y = rect.top() + 48. + (now - a).num_seconds() as f32 / 3600. * hour;
                    p.line_segment(
                        [egui::pos2(x, y), egui::pos2(x + day, y)],
                        egui::Stroke::new(2_f32, Color32::from_rgb(212, 166, 114)),
                    );
                }
            }
        });
    }
    fn tasks(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Flexible queue");
            ui.label(
                RichText::new("Reorder work · deadlines stay attached")
                    .small()
                    .weak(),
            );
        });
        let mut swap = None;
        let mut start = None;
        egui::ScrollArea::vertical()
            .id_salt("tasks")
            .max_height(115.)
            .show(ui, |ui| {
                let len = self.state.tasks.len();
                for (i, t) in self.state.tasks.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut t.done, "").changed() {
                            self.dirty = true;
                        }
                        ui.label(if t.done {
                            RichText::new(&t.title).strikethrough().weak()
                        } else {
                            RichText::new(&t.title)
                        });
                        ui.label(format!("{} min", t.minutes));
                        if let Some(d) = t.deadline {
                            ui.label(format!(
                                "due {}",
                                d.with_timezone(&self.state.timezone).format("%d %b %H:%M")
                            ));
                        }
                        if ui.small_button("↑").clicked() && i > 0 {
                            swap = Some((i, i - 1));
                        }
                        if ui.small_button("↓").clicked() && i + 1 < len {
                            swap = Some((i, i + 1));
                        }
                        if !t.done && ui.small_button("Start").clicked() {
                            start = Some(t.title.clone());
                        }
                    });
                }
            });
        if let Some((a, b)) = swap {
            self.state.tasks.swap(a, b);
            self.state.suggested_order.clear();
            self.state.plan.clear();
            self.dirty = true;
        }
        if let Some(t) = start {
            self.state.start(t, Utc::now());
            self.dirty = true;
        }
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.new_title).hint_text("Add a task"));
            ui.add(
                egui::DragValue::new(&mut self.new_minutes)
                    .range(1..=1440)
                    .suffix(" min"),
            );
            if ui.button("Add").clicked() && !self.new_title.trim().is_empty() {
                self.state.tasks.push(Task {
                    title: std::mem::take(&mut self.new_title),
                    minutes: self.new_minutes,
                    deadline: None,
                    done: false,
                    google_id: None,
                });
                self.dirty = true;
            }
        });
    }
    fn prompt(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.add(egui::TextEdit::multiline(&mut self.prompt).desired_rows(2).desired_width(f32::INFINITY).hint_text("Tell Angela: I worked on Docs for an hour. I need three hours of cardio each week…"));
        ui.horizontal(|ui| {
            let busy = self.receiver.is_some();
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(if self.recorder.is_some() {
                        "Stop & transcribe"
                    } else {
                        "Speak"
                    }),
                )
                .clicked()
            {
                if let Some(child) = self.recorder.take() {
                    let s = self.state.clone();
                    self.job("Transcribing locally with Omaspeak…", move || {
                        Ok(Reply::Transcript(services::transcribe(&s, child)?))
                    });
                } else {
                    match services::record() {
                        Ok(child) => {
                            self.recorder = Some(child);
                            self.status =
                                "Recording · click Stop & transcribe when finished".into();
                        }
                        Err(e) => self.status = format!("{e:#}"),
                    }
                }
            }
            if ui
                .add_enabled(
                    !busy
                        && self.recorder.is_none()
                        && !self.prompt.trim().is_empty()
                        && self.pending.is_none(),
                    egui::Button::new("Interpret"),
                )
                .clicked()
            {
                let s = self.state.clone();
                let text = self.prompt.clone();
                self.pending_report = text.clone();
                self.planning = false;
                self.job("Interpreting with local Ollama…", move || {
                    Ok(Reply::Actions(services::interpret(&s, &text)?))
                });
            }
            if ui.button("Save note").clicked() && !self.prompt.trim().is_empty() {
                self.state.journal.push(std::mem::take(&mut self.prompt));
                self.save();
                self.status = "Note saved".into();
            }
            if busy {
                ui.spinner();
            }
            ui.label(RichText::new(&self.status).small().weak());
        });
    }
    fn windows(&mut self, ctx: &egui::Context) {
        let mut settings = self.settings;
        egui::Window::new("Settings & connections").open(&mut settings).default_width(540.).show(ctx,|ui|{
            ui.heading("Omaspeak");ui.label("Executable path (the fork may still install as handy)");if ui.text_edit_singleline(&mut self.state.omaspeak).changed(){self.dirty=true;}
            ui.horizontal(|ui|{if ui.add_enabled(self.receiver.is_none(),egui::Button::new("Find voice models")).clicked(){let s=self.state.clone();self.job("Reading Omaspeak models…",move||Ok(Reply::VoiceModels(services::voice_models(&s)?)));}
if ui.button("Open Omaspeak / download models").clicked(){if let Err(e)=services::open_voice(&self.state){self.status=format!("{e:#}");}}});
            ui.label("Voice model ID (empty uses Omaspeak's selected model)");if ui.text_edit_singleline(&mut self.state.voice_model).changed(){self.dirty=true;}
            if let Some(models)=self.voice_models.as_array(){egui::ScrollArea::vertical().max_height(100.).show(ui,|ui|{for m in models{let id=m["id"].as_str().unwrap_or_default();let installed=m["is_downloaded"].as_bool().or_else(||m["downloaded"].as_bool()).unwrap_or(false);if ui.selectable_label(self.state.voice_model==id,format!("{id}{}",if installed{" · installed"}else{""})).clicked(){self.state.voice_model=id.into();self.dirty=true;}}});}
            ui.separator();ui.heading("Ollama");ui.label("Local endpoint");if ui.text_edit_singleline(&mut self.state.ollama_url).changed(){self.dirty=true;}ui.label("Planning / interpretation model");if ui.text_edit_singleline(&mut self.state.ollama_model).changed(){self.dirty=true;}
            ui.horizontal(|ui|{if ui.add_enabled(self.receiver.is_none(),egui::Button::new("List models")).clicked(){let s=self.state.clone();self.job("Reading Ollama models…",move||Ok(Reply::Models(services::ollama_models(&s)?)));}
if ui.add_enabled(self.receiver.is_none(),egui::Button::new("Download model")).clicked(){let s=self.state.clone();self.job("Downloading Ollama model…",move||{services::pull(&s)?;Ok(Reply::Message("Ollama model ready".into()))});}});
            for m in &self.models {if ui.selectable_label(self.state.ollama_model==*m,m).clicked(){self.state.ollama_model=m.clone();self.dirty=true;}}
            ui.separator();ui.heading("Google Calendar");ui.label("Desktop app OAuth credentials JSON path");if ui.text_edit_singleline(&mut self.state.google_credentials).changed(){self.dirty=true;}
            ui.horizontal(|ui|{if ui.add_enabled(self.receiver.is_none(),egui::Button::new("Connect Google")).clicked(){self.save();let path=self.state.google_credentials.clone();self.job("Finish Google sign-in in your browser…",move||{google::connect(&path)?;Ok(Reply::Connected)});}
if ui.add_enabled(self.receiver.is_none(),egui::Button::new("Disconnect")).clicked(){match google::disconnect(){Ok(())=>{self.state.events.clear();self.state.calendars.clear();self.state.last_sync=None;self.save();self.status="Google disconnected".into();},Err(e)=>self.status=e.to_string()}}});
            ui.label("Calendar commitments sync into Angela. Your activity log and proposed schedule stay local.");
            ui.label(self.state.last_sync.map(|t|format!("Last sync: {}",t.with_timezone(&self.state.timezone).format("%d %b %H:%M"))).unwrap_or("Not synced yet".into()));
            ui.separator();ui.label(format!("Timezone: {}",self.state.timezone));ui.label(RichText::new(&self.status).small());
        });
        self.settings = settings;
        let mut requirements = self.requirements;
        egui::Window::new("Requirements")
            .open(&mut requirements)
            .default_width(570.)
            .show(ctx, |ui| {
                ui.label(
                    "Minimum time targets. Use the prompt for new requirements and deadlines.",
                );
                let mut remove = None;
                for (i, r) in self.state.requirements.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut r.enabled, "").changed() {
                            self.dirty = true;
                        }
                        if ui
                            .add(egui::TextEdit::singleline(&mut r.title).desired_width(140.))
                            .changed()
                        {
                            self.dirty = true;
                        }
                        if ui
                            .add(
                                egui::DragValue::new(&mut r.minutes)
                                    .range(1..=10080)
                                    .suffix(" min"),
                            )
                            .changed()
                        {
                            self.dirty = true;
                        }
                        if ui.checkbox(&mut r.weekly, "weekly").changed() {
                            self.dirty = true;
                        }
                        if ui.small_button("×").clicked() {
                            remove = Some(i);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Days");
                        for (d, label) in ["M", "Tu", "W", "Th", "F", "Sa", "Su"]
                            .into_iter()
                            .enumerate()
                        {
                            let mut yes = r.days.is_empty() || r.days.contains(&(d as u32));
                            if ui.checkbox(&mut yes, label).changed() {
                                if r.days.is_empty() {
                                    r.days = (0..7).collect();
                                }
                                if yes {
                                    r.days.push(d as u32);
                                    r.days.sort();
                                    r.days.dedup();
                                } else {
                                    r.days.retain(|v| *v != d as u32);
                                    if r.days.is_empty() {
                                        r.enabled = false;
                                    }
                                }
                                self.dirty = true;
                            }
                        }
                    });
                }
                if let Some(i) = remove {
                    self.state.requirements.remove(i);
                    self.dirty = true;
                }
                if ui.button("Add requirement").clicked() {
                    self.state.requirements.push(Requirement {
                        title: "New requirement".into(),
                        minutes: 60,
                        weekly: false,
                        enabled: true,
                        days: vec![],
                    });
                    self.dirty = true;
                }
            });
        self.requirements = requirements;
        if let Some(a) = self.pending.clone() {
            let mut open = true;
            egui::Window::new("Review interpretation")
                .open(&mut open)
                .default_width(520.)
                .show(ctx, |ui| {
                    ui.label("Apply these changes to your local schedule?");
                    for action in &a {
                        ui.label(match action {
                            Action::Start { title } => format!("Start tracking {title}"),
                            Action::Stop => "Stop the current activity".into(),
                            Action::Sleep => "Start tracking sleep".into(),
                            Action::Wake => "End the current sleep / activity".into(),
                            Action::Log { title, start, end } => format!(
                                "Log {title}: {} – {}",
                                start
                                    .with_timezone(&self.state.timezone)
                                    .format("%d %b %H:%M"),
                                end.with_timezone(&self.state.timezone)
                                    .format("%d %b %H:%M")
                            ),
                            Action::Task {
                                title,
                                minutes,
                                deadline,
                            } => format!(
                                "Add {title} · {minutes} min{}",
                                deadline
                                    .map(|d| format!(
                                        " · due {}",
                                        d.with_timezone(&self.state.timezone).format("%d %b %H:%M")
                                    ))
                                    .unwrap_or_default()
                            ),
                            Action::Requirement {
                                title,
                                minutes,
                                weekly,
                            } => format!(
                                "Require {title} · {minutes} min per {}",
                                if *weekly { "week" } else { "day" }
                            ),
                            Action::Plan { order } => format!("Work order: {}", order.join(" → ")),
                        });
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Apply").clicked() {
                            match domain::apply(&mut self.state, &a, Utc::now()) {
                                Ok(()) => {
                                    if self.planning {
                                        self.state.fill(Utc::now(), self.plan_hours);
                                        self.planning = false;
                                    }
                                    self.state
                                        .journal
                                        .push(std::mem::take(&mut self.pending_report));
                                    self.prompt.clear();
                                    self.pending = None;
                                    self.save();
                                    self.status = "Schedule updated".into();
                                }
                                Err(e) => self.status = e.to_string(),
                            }
                        }
                        if ui.button("Discard").clicked() {
                            self.pending = None;
                        }
                    });
                });
            if !open {
                self.pending = None;
            }
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        self.side(ctx);
        egui::CentralPanel::default().show(ctx, |ui| {
            self.calendar(ui);
            ui.add_space(12.);
            self.tasks(ui);
            self.prompt(ui);
        });
        self.windows(ctx);
        if self.dirty {
            self.state.plan.clear();
            self.save();
        }
        if self.last_tick.elapsed() > Wait::from_secs(300) && self.receiver.is_none() {
            self.last_tick = std::time::Instant::now();
            if self.state.last_sync.is_some() {
                self.sync();
            }
        }
        ctx.request_repaint_after(Wait::from_secs(1));
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(mut child) = self.recorder.take() {
            let _ = child.kill();
            let _ = child.wait();
            if let Ok(p) = storage::directory() {
                let _ = std::fs::remove_file(p.join("recording.wav"));
            }
        }
        self.save();
    }
}
fn main() -> anyhow::Result<()> {
    let state = storage::load()?;
    eframe::run_native(
        "Angela",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1280., 900.])
                .with_min_inner_size([1040., 780.]),
            ..Default::default()
        },
        Box::new(move |cc| Ok(Box::new(App::new(&cc.egui_ctx, state)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod ui_tests {
    use super::*;
    #[test]
    fn desktop_layout_and_settings_render_without_a_display() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, State::default());
        app.settings = true;
        app.requirements = true;
        app.pending = Some(vec![Action::Sleep]);
        for _ in 0..2 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280., 900.),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    app.side(ctx);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        app.calendar(ui);
                        app.tasks(ui);
                        app.prompt(ui);
                    });
                    app.windows(ctx);
                },
            );
            assert!(!output.shapes.is_empty());
            assert!(!ctx
                .tessellate(output.shapes, output.pixels_per_point)
                .is_empty());
        }
    }
}

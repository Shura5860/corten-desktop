#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PomodoroMode {
    Focus,
    ShortBreak,
}

impl PomodoroMode {
    pub fn default_duration_secs(&self) -> u32 {
        match self {
            Self::Focus => 25 * 60,
            Self::ShortBreak => 5 * 60,
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            Self::Focus => "Odaklanma",
            Self::ShortBreak => "Kısa Mola",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PomodoroTimer {
    pub mode: PomodoroMode,
    pub is_running: bool,
    pub remaining_secs: u32,
    pub completed_sessions: u32,
}

impl Default for PomodoroTimer {
    fn default() -> Self {
        Self {
            mode: PomodoroMode::Focus,
            is_running: false,
            remaining_secs: PomodoroMode::Focus.default_duration_secs(),
            completed_sessions: 0,
        }
    }
}

impl PomodoroTimer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tick(&mut self) -> bool {
        if self.is_running && self.remaining_secs > 0 {
            self.remaining_secs -= 1;
            if self.remaining_secs == 0 {
                self.is_running = false;
                if self.mode == PomodoroMode::Focus {
                    self.completed_sessions += 1;
                    self.mode = PomodoroMode::ShortBreak;
                } else {
                    self.mode = PomodoroMode::Focus;
                }
                self.remaining_secs = self.mode.default_duration_secs();
                return true; // Notification trigger
            }
        }
        false
    }

    pub fn toggle_run(&mut self) {
        self.is_running = !self.is_running;
    }

    pub fn reset(&mut self) {
        self.is_running = false;
        self.remaining_secs = self.mode.default_duration_secs();
    }

    pub fn switch_mode(&mut self) {
        self.is_running = false;
        self.mode = match self.mode {
            PomodoroMode::Focus => PomodoroMode::ShortBreak,
            PomodoroMode::ShortBreak => PomodoroMode::Focus,
        };
        self.remaining_secs = self.mode.default_duration_secs();
    }

    pub fn formatted_time(&self) -> String {
        let mins = self.remaining_secs / 60;
        let secs = self.remaining_secs % 60;
        format!("{:02}:{:02}", mins, secs)
    }

    pub fn progress(&self) -> f32 {
        let total = self.mode.default_duration_secs() as f32;
        let left = self.remaining_secs as f32;
        1.0 - (left / total).clamp(0.0, 1.0)
    }
}

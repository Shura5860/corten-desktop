use iced::mouse;
use iced::widget::canvas::{self, Canvas, Geometry, Path, Program, Stroke};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};
use std::f32::consts::PI;

#[derive(Debug, Clone)]
pub struct WaveformVisualizer {
    phase: f32,
    is_playing: bool,
    bar_heights: Vec<f32>,
}

impl WaveformVisualizer {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            is_playing: false,
            bar_heights: vec![0.2; 28],
        }
    }

    pub fn tick(&mut self, is_playing: bool) {
        self.is_playing = is_playing;
        if is_playing {
            self.phase += 0.22;
            if self.phase > 2.0 * PI * 100.0 {
                self.phase = 0.0;
            }
            // Generate rhythmic, fluid audio equalizer bar amplitudes matching concept wave
            for (i, bar) in self.bar_heights.iter_mut().enumerate() {
                let idx = i as f32;
                // Shape peak around bar 5-7 and smoothly decay towards right
                let envelope = (-(idx - 6.0).powi(2) / 120.0).exp();
                let wave1 = (self.phase * 1.8 + idx * 0.45).sin();
                let wave2 = (self.phase * 2.6 - idx * 0.35).cos();
                let beat = ((wave1 + wave2) * 0.5).abs() * envelope;
                *bar = (*bar * 0.6) + (beat.clamp(0.12, 1.0) * 0.4);
            }
        } else {
            // Concept idle baseline wave
            for (i, bar) in self.bar_heights.iter_mut().enumerate() {
                let idx = i as f32;
                let static_envelope = (-(idx - 6.0).powi(2) / 100.0).exp();
                *bar = (*bar * 0.85).max(0.15 * static_envelope).max(0.06);
            }
        }
    }

    pub fn view<'a, Message: 'a>(&'a self, primary_color: Color) -> Element<'a, Message> {
        Canvas::new(VisualizerProgram {
            bar_heights: &self.bar_heights,
            primary_color,
            is_playing: self.is_playing,
        })
        .width(Length::Fixed(210.0))
        .height(Length::Fixed(50.0))
        .into()
    }
}

struct VisualizerProgram<'a> {
    bar_heights: &'a [f32],
    primary_color: Color,
    is_playing: bool,
}

impl<'a, Message> Program<Message> for VisualizerProgram<'a> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        let total_bars = self.bar_heights.len();
        let total_width = bounds.width;
        let gap = 3.0;
        let bar_width = (total_width - (gap * (total_bars as f32 - 1.0))) / total_bars as f32;
        let max_height = bounds.height - 4.0;

        for (i, &factor) in self.bar_heights.iter().enumerate() {
            let x = i as f32 * (bar_width + gap);
            let bar_h = (factor * max_height).max(4.0);
            let y = bounds.height - bar_h;

            let path =
                Path::rounded_rectangle(Point::new(x, y), Size::new(bar_width, bar_h), 2.0.into());

            // Color matching concept: warm cream/beige (#E8C39E) at upper heights, warm rust at lower
            let cream = Color::from_rgb(0.91, 0.76, 0.62);
            let rust = Color::from_rgb(0.85, 0.47, 0.21);

            let t = factor.clamp(0.0, 1.0);
            let color = Color {
                r: rust.r + (cream.r - rust.r) * t,
                g: rust.g + (cream.g - rust.g) * t,
                b: rust.b + (cream.b - rust.b) * t,
                a: ((0.75 + t * 0.25) * if self.is_playing { 1.0 } else { 0.65 }).min(1.0),
            };

            frame.fill(&path, color);
        }

        // Draw subtle bottom baseline glow
        let baseline = Path::line(
            Point::new(0.0, bounds.height - 1.0),
            Point::new(bounds.width, bounds.height - 1.0),
        );
        frame.stroke(
            &baseline,
            Stroke::default()
                .with_color(Color {
                    a: 0.25,
                    ..self.primary_color
                })
                .with_width(1.0),
        );

        vec![frame.into_geometry()]
    }
}

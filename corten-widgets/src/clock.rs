use chrono::{DateTime, Local, Timelike, Utc};
use corten_theme::Palette;
use iced::mouse;
use iced::widget::canvas::{self, Canvas, Frame, Geometry, Path, Program, Stroke};
use iced::{Color, Element, Point, Rectangle, Renderer, Theme};

pub struct AnalogClockCanvas {
    pub time: DateTime<Local>,
    pub palette: Palette,
}

impl<Message> Program<Message> for AnalogClockCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let radius = (bounds.width.min(bounds.height) / 2.0) - 3.0;

        if radius <= 10.0 {
            return vec![frame.into_geometry()];
        }

        // 1. Dial Face with Vertical Smoked Gradient + Smooth Filled Outer Rim (zero stroke aliasing)
        let outer_bezel = Path::circle(center, radius);
        let bezel_grad = canvas::gradient::Linear::new(
            Point::new(center.x, center.y - radius),
            Point::new(center.x, center.y + radius),
        )
        .add_stop(0.0, Color::from_rgba(1.0, 1.0, 1.0, 0.14))
        .add_stop(0.5, Color::from_rgba(1.0, 1.0, 1.0, 0.06))
        .add_stop(1.0, Color::from_rgba(1.0, 1.0, 1.0, 0.03));
        frame.fill(&outer_bezel, bezel_grad);

        let inner_face = Path::circle(center, (radius - 1.0).max(1.0));
        let dial_grad = canvas::gradient::Linear::new(
            Point::new(center.x, center.y - radius),
            Point::new(center.x, center.y + radius),
        )
        .add_stop(0.0, Color::from_rgba(0.11, 0.12, 0.15, 0.96))
        .add_stop(1.0, Color::from_rgba(0.04, 0.05, 0.07, 0.98));
        frame.fill(&inner_face, dial_grad);

        // 2. Clean 12 Hour Markers + Subtle 5-Minute Sub-ticks (avoids 60-tick pixel crowding)
        for i in 0..12 {
            let angle = (i as f32) * (std::f32::consts::PI / 6.0);
            let cos = angle.cos();
            let sin = angle.sin();

            let is_cardinal = i % 3 == 0;
            let tick_len = if is_cardinal {
                radius * 0.20
            } else {
                radius * 0.13
            };
            let width = if is_cardinal { 1.6 } else { 1.1 };

            let start_r = radius * 0.86;
            let end_r = start_r - tick_len;

            let p1 = Point::new(center.x + sin * start_r, center.y - cos * start_r);
            let p2 = Point::new(center.x + sin * end_r, center.y - cos * end_r);

            let tick_color = if is_cardinal {
                Color::from_rgba(
                    self.palette.primary.r,
                    self.palette.primary.g,
                    self.palette.primary.b,
                    0.90,
                )
            } else {
                Color::from_rgba(0.86, 0.88, 0.92, 0.50)
            };

            frame.stroke(
                &Path::line(p1, p2),
                Stroke::default()
                    .with_color(tick_color)
                    .with_width(width)
                    .with_line_cap(canvas::LineCap::Round),
            );
        }

        // Time values
        let hour = self.time.hour() % 12;
        let minute = self.time.minute();
        let second = self.time.second();

        // 3. Hour Hand
        let hour_angle = ((hour as f32) + (minute as f32 / 60.0)) * (std::f32::consts::PI / 6.0);
        let hour_len = radius * 0.50;
        let hp2 = Point::new(
            center.x + hour_angle.sin() * hour_len,
            center.y - hour_angle.cos() * hour_len,
        );
        frame.stroke(
            &Path::line(center, hp2),
            Stroke::default()
                .with_color(Color::from_rgba(0.95, 0.96, 0.98, 0.94))
                .with_width(2.4)
                .with_line_cap(canvas::LineCap::Round),
        );

        // 4. Minute Hand
        let min_angle = ((minute as f32) + (second as f32 / 60.0)) * (std::f32::consts::PI / 30.0);
        let min_len = radius * 0.72;
        let mp2 = Point::new(
            center.x + min_angle.sin() * min_len,
            center.y - min_angle.cos() * min_len,
        );
        frame.stroke(
            &Path::line(center, mp2),
            Stroke::default()
                .with_color(Color::from_rgba(0.88, 0.90, 0.94, 0.92))
                .with_width(1.7)
                .with_line_cap(canvas::LineCap::Round),
        );

        // 5. Second Hand (Glowing Copper)
        let sec_angle = (second as f32) * (std::f32::consts::PI / 30.0);
        let sec_len = radius * 0.78;
        let sp_tail = Point::new(
            center.x - sec_angle.sin() * (radius * 0.18),
            center.y + sec_angle.cos() * (radius * 0.18),
        );
        let sp_head = Point::new(
            center.x + sec_angle.sin() * sec_len,
            center.y - sec_angle.cos() * sec_len,
        );
        frame.stroke(
            &Path::line(sp_tail, sp_head),
            Stroke::default()
                .with_color(self.palette.primary.to_iced())
                .with_width(1.2)
                .with_line_cap(canvas::LineCap::Round),
        );

        // 6. Central Copper Cap
        let center_circle = Path::circle(center, 3.0);
        frame.fill(&center_circle, self.palette.primary.to_iced());
        let center_pin = Path::circle(center, 1.1);
        frame.fill(&center_pin, Color::from_rgba(0.06, 0.07, 0.09, 0.95));

        vec![frame.into_geometry()]
    }
}

pub struct ClockData {
    pub local_time: DateTime<Local>,
    #[allow(dead_code)]
    pub utc_time: DateTime<Utc>,
}

impl ClockData {
    pub fn now() -> Self {
        Self {
            local_time: Local::now(),
            utc_time: Utc::now(),
        }
    }

    pub fn view<'a, Message: 'static>(&self, palette: Palette) -> Element<'a, Message> {
        Canvas::new(AnalogClockCanvas {
            time: self.local_time,
            palette,
        })
        .width(iced::Length::Fixed(84.0))
        .height(iced::Length::Fixed(84.0))
        .into()
    }
}

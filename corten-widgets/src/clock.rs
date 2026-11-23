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
        let radius = (bounds.width.min(bounds.height) / 2.0) - 4.0;

        if radius <= 10.0 {
            return vec![frame.into_geometry()];
        }

        // 1. Dial Outer Ring (Forged Steel Bezel)
        let outer_circle = Path::circle(center, radius);
        frame.fill(&outer_circle, Color::from_rgba(0.06, 0.07, 0.09, 0.95));
        frame.stroke(
            &outer_circle,
            Stroke::default()
                .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.12))
                .with_width(1.5),
        );

        // Subtle inner ring
        let inner_circle = Path::circle(center, radius * 0.92);
        frame.stroke(
            &inner_circle,
            Stroke::default()
                .with_color(Color::from_rgba(
                    self.palette.primary.r,
                    self.palette.primary.g,
                    self.palette.primary.b,
                    0.25,
                ))
                .with_width(1.0),
        );

        // 2. Hour & Minute Ticks
        for i in 0..60 {
            let angle = (i as f32) * (std::f32::consts::PI / 30.0);
            let cos = angle.cos();
            let sin = angle.sin();

            let is_hour = i % 5 == 0;
            let tick_len = if is_hour {
                radius * 0.18
            } else {
                radius * 0.08
            };
            let width = if is_hour { 2.0 } else { 0.8 };

            let start_r = radius * 0.88;
            let end_r = start_r - tick_len;

            let p1 = Point::new(center.x + sin * start_r, center.y - cos * start_r);
            let p2 = Point::new(center.x + sin * end_r, center.y - cos * end_r);

            let tick_color = if is_hour {
                if i % 15 == 0 {
                    Color::from_rgba(
                        self.palette.primary.r,
                        self.palette.primary.g,
                        self.palette.primary.b,
                        0.9,
                    )
                } else {
                    Color::from_rgba(0.9, 0.92, 0.95, 0.75)
                }
            } else {
                Color::from_rgba(1.0, 1.0, 1.0, 0.2)
            };

            frame.stroke(
                &Path::line(p1, p2),
                Stroke::default().with_color(tick_color).with_width(width),
            );
        }

        // Time values
        let hour = self.time.hour() % 12;
        let minute = self.time.minute();
        let second = self.time.second();

        // 3. Hour Hand
        let hour_angle = ((hour as f32) + (minute as f32 / 60.0)) * (std::f32::consts::PI / 6.0);
        let hour_len = radius * 0.52;
        let hp2 = Point::new(
            center.x + hour_angle.sin() * hour_len,
            center.y - hour_angle.cos() * hour_len,
        );
        frame.stroke(
            &Path::line(center, hp2),
            Stroke::default()
                .with_color(Color::from_rgba(0.92, 0.94, 0.96, 0.9))
                .with_width(3.0)
                .with_line_cap(canvas::LineCap::Round),
        );

        // 4. Minute Hand
        let min_angle = ((minute as f32) + (second as f32 / 60.0)) * (std::f32::consts::PI / 30.0);
        let min_len = radius * 0.74;
        let mp2 = Point::new(
            center.x + min_angle.sin() * min_len,
            center.y - min_angle.cos() * min_len,
        );
        frame.stroke(
            &Path::line(center, mp2),
            Stroke::default()
                .with_color(Color::from_rgba(0.85, 0.88, 0.92, 0.95))
                .with_width(2.0)
                .with_line_cap(canvas::LineCap::Round),
        );

        // 5. Second Hand (Patina Copper / Amber)
        let sec_angle = (second as f32) * (std::f32::consts::PI / 30.0);
        let sec_len = radius * 0.82;
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

        // 6. Central Brass Cap Rivet
        let center_circle = Path::circle(center, 3.5);
        frame.fill(&center_circle, self.palette.primary.to_iced());
        let center_pin = Path::circle(center, 1.2);
        frame.fill(&center_pin, Color::from_rgba(0.1, 0.1, 0.1, 0.9));

        vec![frame.into_geometry()]
    }
}

pub struct ClockData {
    pub local_time: DateTime<Local>,
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
        .width(iced::Length::Fixed(92.0))
        .height(iced::Length::Fixed(92.0))
        .into()
    }
}

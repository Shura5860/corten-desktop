use iced::mouse;
use iced::widget::canvas::{self, Canvas, Geometry, Path, Program, Stroke};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Theme};
use std::f32::consts::PI;

pub struct RadialGauge {
    pub percentage: f32, // 0.0 - 100.0
    pub label: String,
    pub value_str: String,
    pub accent_color: Color,
    pub size: f32,
}

impl RadialGauge {
    pub fn new(
        percentage: f32,
        label: impl Into<String>,
        value_str: impl Into<String>,
        accent_color: Color,
    ) -> Self {
        Self {
            percentage: percentage.clamp(0.0, 100.0),
            label: label.into(),
            value_str: value_str.into(),
            accent_color,
            size: 100.0,
        }
    }

    pub fn view<Message: 'static>(self) -> Element<'static, Message> {
        let size = self.size;
        Canvas::new(RadialGaugeProgram {
            percentage: self.percentage,
            label: self.label,
            value_str: self.value_str,
            accent_color: self.accent_color,
        })
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .into()
    }
}

struct RadialGaugeProgram {
    percentage: f32,
    label: String,
    value_str: String,
    accent_color: Color,
}

impl<Message> Program<Message> for RadialGaugeProgram {
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
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let radius = (bounds.width.min(bounds.height) / 2.0) - 8.0;

        // 1. Subtle inner well with vertical smoked gradient
        let well_path = Path::circle(center, radius - 3.5);
        let well_grad = canvas::gradient::Linear::new(
            Point::new(center.x, center.y - radius),
            Point::new(center.x, center.y + radius),
        )
        .add_stop(0.0, Color::from_rgba(0.05, 0.06, 0.08, 0.55))
        .add_stop(1.0, Color::from_rgba(0.09, 0.10, 0.13, 0.25));
        frame.fill(&well_path, well_grad);

        // 2. Background Track (clean single dark-bronze ring)
        let bg_path = Path::circle(center, radius);
        frame.stroke(
            &bg_path,
            Stroke::default()
                .with_color(Color::from_rgba(
                    self.accent_color.r,
                    self.accent_color.g,
                    self.accent_color.b,
                    0.18,
                ))
                .with_width(6.0),
        );

        // 3. Foreground Progress Arc with Smooth Linear Copper-Amber Gradient (no muddy double stroke)
        let start_angle = iced::Radians(-PI / 2.0);
        let sweep_angle = (self.percentage / 100.0) * 2.0 * PI;
        let end_angle = iced::Radians((-PI / 2.0) + sweep_angle);

        if sweep_angle > 0.005 {
            let fg_path = Path::new(|builder| {
                builder.arc(canvas::path::Arc {
                    center,
                    radius,
                    start_angle,
                    end_angle,
                });
            });

            let bright_amber = Color::from_rgba(
                (self.accent_color.r + 0.16).min(1.0),
                (self.accent_color.g + 0.22).min(1.0),
                (self.accent_color.b + 0.16).min(1.0),
                1.0,
            );
            let deep_copper = Color::from_rgba(
                (self.accent_color.r * 0.85).max(0.0),
                (self.accent_color.g * 0.78).max(0.0),
                (self.accent_color.b * 0.72).max(0.0),
                0.95,
            );

            let arc_grad = canvas::gradient::Linear::new(
                Point::new(center.x - radius, center.y - radius),
                Point::new(center.x + radius, center.y + radius),
            )
            .add_stop(0.0, bright_amber)
            .add_stop(0.55, self.accent_color)
            .add_stop(1.0, deep_copper);

            frame.stroke(
                &fg_path,
                Stroke {
                    style: canvas::Style::Gradient(canvas::Gradient::Linear(arc_grad)),
                    width: 6.0,
                    line_cap: canvas::LineCap::Round,
                    ..Stroke::default()
                },
            );
        }

        // 4. Clean Light/Regular Center Typography
        let label_color = Color::from_rgba(0.62, 0.65, 0.70, 0.92);
        let text_color = Color::from_rgba(0.96, 0.97, 0.98, 0.98);

        frame.fill_text(canvas::Text {
            content: self.label.to_string(),
            position: Point::new(center.x, center.y - 10.5),
            color: label_color,
            size: iced::Pixels(10.5),
            font: corten_theme::NOTO_SANS,
            horizontal_alignment: iced::alignment::Horizontal::Center,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..Default::default()
        });

        frame.fill_text(canvas::Text {
            content: self.value_str.to_string(),
            position: Point::new(center.x, center.y + 8.5),
            color: text_color,
            size: iced::Pixels(17.5),
            font: corten_theme::NOTO_SANS,
            horizontal_alignment: iced::alignment::Horizontal::Center,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..Default::default()
        });

        vec![frame.into_geometry()]
    }
}

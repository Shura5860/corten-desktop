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
            size: 94.0,
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

        // 1. Frosted circular background plate with subtle glowing glass rim
        let plate_path = Path::circle(center, radius + 6.0);
        frame.fill(&plate_path, Color::from_rgba(0.08, 0.09, 0.11, 0.90));
        frame.stroke(
            &plate_path,
            Stroke::default()
                .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.08))
                .with_width(1.0),
        );

        // 2. Background Track (dim translucent ring)
        let bg_path = Path::circle(center, radius);
        frame.stroke(
            &bg_path,
            Stroke::default()
                .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.10))
                .with_width(7.5),
        );

        // 3. Foreground Progress Arc (starts from top -PI/2)
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

            frame.stroke(
                &fg_path,
                Stroke::default()
                    .with_color(self.accent_color)
                    .with_width(7.5),
            );

            // Glowing indicator head (round dot at progress tip)
            let tip_x = center.x + radius * ((-PI / 2.0) + sweep_angle).cos();
            let tip_y = center.y + radius * ((-PI / 2.0) + sweep_angle).sin();
            let tip_center = Point::new(tip_x, tip_y);
            let tip_path = Path::circle(tip_center, 4.0);
            frame.fill(&tip_path, Color::from_rgb(1.0, 1.0, 1.0));
        }

        // 4. Center Text: Label on TOP ("CPU" / "RAM"), Value on BOTTOM ("34%" / "52%") matching concept drawing
        let label_color = Color::from_rgb(0.60, 0.62, 0.68);
        let text_color = Color::from_rgb(1.0, 1.0, 1.0);

        frame.fill_text(canvas::Text {
            content: self.label.to_string(),
            position: Point::new(center.x, center.y - 10.0),
            color: label_color,
            size: iced::Pixels(11.0),
            font: iced::Font::DEFAULT,
            horizontal_alignment: iced::alignment::Horizontal::Center,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..Default::default()
        });

        frame.fill_text(canvas::Text {
            content: self.value_str.to_string(),
            position: Point::new(center.x, center.y + 9.0),
            color: text_color,
            size: iced::Pixels(16.0),
            font: iced::Font::DEFAULT,
            horizontal_alignment: iced::alignment::Horizontal::Center,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..Default::default()
        });

        vec![frame.into_geometry()]
    }
}

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::{Element, Kind, RenderElement};
use smithay::backend::renderer::gles::element::{PixelShaderElement, TextureShaderElement};
use smithay::backend::renderer::gles::{
    GlesError, GlesPixelProgram, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
    UniformType, UniformValue,
};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{Bind, Color32F, Frame, Offscreen, Renderer};
use smithay::utils::{Buffer as BufferCoord, Logical, Physical, Point, Rectangle, Size, Transform};
use std::borrow::{BorrowMut, Cow};

pub const DUAL_KAWASE_DOWN_SRC: &str = r#"
precision mediump float;
varying vec2 v_coords;
uniform sampler2D tex;
uniform float alpha;
uniform vec2 u_halfpixel;

void main() {
    vec4 sum = texture2D(tex, v_coords) * 4.0;
    sum += texture2D(tex, v_coords - u_halfpixel);
    sum += texture2D(tex, v_coords + u_halfpixel);
    sum += texture2D(tex, v_coords + vec2(u_halfpixel.x, -u_halfpixel.y));
    sum += texture2D(tex, v_coords - vec2(u_halfpixel.x, -u_halfpixel.y));
    gl_FragColor = (sum / 8.0) * alpha;
}
"#;

pub const DUAL_KAWASE_UP_SRC: &str = r#"
precision mediump float;
varying vec2 v_coords;
uniform sampler2D tex;
uniform float alpha;
uniform vec2 u_halfpixel;
uniform float u_tint;
uniform vec4 u_region;
uniform vec2 u_size;
uniform float u_radius;

float rounded_box(vec2 p, vec2 b, float r) {
    vec2 d = abs(p) - b + vec2(r);
    return min(max(d.x, d.y), 0.0) + length(max(d, 0.0)) - r;
}

void main() {
    float mask = 1.0;
    if (u_radius > 0.5 && u_region.z > 0.0001 && u_region.w > 0.0001) {
        vec2 norm_coord = (v_coords - u_region.xy) / u_region.zw;
        vec2 p = (norm_coord - 0.5) * u_size;
        vec2 b = u_size * 0.5;
        float dist = rounded_box(p, b, u_radius);
        mask = 1.0 - smoothstep(-0.8, 0.8, dist);
        if (mask <= 0.001) {
            discard;
        }
    }

    vec4 sum = texture2D(tex, v_coords + vec2(-u_halfpixel.x * 2.0, 0.0));
    sum += texture2D(tex, v_coords + vec2(-u_halfpixel.x, u_halfpixel.y)) * 2.0;
    sum += texture2D(tex, v_coords + vec2(0.0, u_halfpixel.y * 2.0));
    sum += texture2D(tex, v_coords + vec2(u_halfpixel.x, u_halfpixel.y)) * 2.0;
    sum += texture2D(tex, v_coords + vec2(u_halfpixel.x * 2.0, 0.0));
    sum += texture2D(tex, v_coords + vec2(u_halfpixel.x, -u_halfpixel.y)) * 2.0;
    sum += texture2D(tex, v_coords + vec2(0.0, -u_halfpixel.y * 2.0));
    sum += texture2D(tex, v_coords + vec2(-u_halfpixel.x, -u_halfpixel.y)) * 2.0;
    vec4 blurred = sum / 12.0;
    vec3 frosted = mix(blurred.rgb, vec3(0.09, 0.10, 0.12), u_tint);
    gl_FragColor = vec4(frosted, 1.0) * alpha * mask;
}
"#;

pub const NIGHT_LIGHT_SRC: &str = r#"
precision mediump float;
varying vec2 v_coords;
uniform sampler2D tex;
uniform float alpha;
uniform float u_warmth;

void main() {
    vec4 col = texture2D(tex, v_coords);
    // Relative luminance (Rec.709) so pure blacks stay true black
    float luma = dot(col.rgb, vec3(0.2126, 0.7152, 0.0722));
    // Smooth luma mask: 0.0 at pure black, 1.0 across midtones/highlights
    float mask = smoothstep(0.02, 0.25, luma) * u_warmth;
    // Kelvin ~4100K warm shift: preserve red, gently warm green, attenuate blue
    vec3 warmed = vec3(
        min(col.r * (1.0 + 0.08 * mask), 1.0),
        col.g * (1.0 - 0.09 * mask),
        col.b * (1.0 - 0.34 * mask)
    );
    gl_FragColor = vec4(warmed, col.a) * alpha;
}
"#;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlassRegion {
    pub rect: Rectangle<i32, Logical>,
    pub corner_radius: f32,
    pub tint: f32,
}

impl GlassRegion {
    pub fn new(rect: Rectangle<i32, Logical>, corner_radius: f32, tint: f32) -> Self {
        Self {
            rect,
            corner_radius,
            tint,
        }
    }

    pub fn window(rect: Rectangle<i32, Logical>) -> Self {
        Self {
            rect,
            corner_radius: 8.0,
            tint: 0.22,
        }
    }

    pub fn panel(rect: Rectangle<i32, Logical>) -> Self {
        Self {
            rect,
            corner_radius: 16.0,
            tint: 0.32,
        }
    }
}

impl From<Rectangle<i32, Logical>> for GlassRegion {
    fn from(rect: Rectangle<i32, Logical>) -> Self {
        Self::window(rect)
    }
}

pub struct DualKawaseBlurPipeline {
    down_program: GlesTexProgram,
    up_program: GlesTexProgram,
    night_light_program: GlesTexProgram,
    scene_tex: Option<(Size<i32, BufferCoord>, GlesTexture)>,
    half_tex: Option<(Size<i32, BufferCoord>, GlesTexture)>,
    quarter_tex: Option<(Size<i32, BufferCoord>, GlesTexture)>,
    commit: CommitCounter,
}

impl DualKawaseBlurPipeline {
    pub fn new(renderer: &mut GlesRenderer) -> Result<Self, GlesError> {
        let down_uniforms = [UniformName::new("u_halfpixel", UniformType::_2f)];
        let up_uniforms = [
            UniformName::new("u_halfpixel", UniformType::_2f),
            UniformName::new("u_tint", UniformType::_1f),
            UniformName::new("u_region", UniformType::_4f),
            UniformName::new("u_size", UniformType::_2f),
            UniformName::new("u_radius", UniformType::_1f),
        ];
        let nl_uniforms = [UniformName::new("u_warmth", UniformType::_1f)];
        let down_program =
            renderer.compile_custom_texture_shader(DUAL_KAWASE_DOWN_SRC, &down_uniforms)?;
        let up_program =
            renderer.compile_custom_texture_shader(DUAL_KAWASE_UP_SRC, &up_uniforms)?;
        let night_light_program =
            renderer.compile_custom_texture_shader(NIGHT_LIGHT_SRC, &nl_uniforms)?;
        Ok(Self {
            down_program,
            up_program,
            night_light_program,
            scene_tex: None,
            half_tex: None,
            quarter_tex: None,
            commit: CommitCounter::default(),
        })
    }

    fn ensure_textures(
        &mut self,
        renderer: &mut GlowRenderer,
        out_w: i32,
        out_h: i32,
    ) -> Result<(), GlesError> {
        let full_size: Size<i32, BufferCoord> = (out_w.max(4), out_h.max(4)).into();
        let half_size: Size<i32, BufferCoord> = ((out_w / 2).max(2), (out_h / 2).max(2)).into();
        let quarter_size: Size<i32, BufferCoord> = ((out_w / 4).max(1), (out_h / 4).max(1)).into();

        if self.scene_tex.as_ref().map(|(s, _)| *s) != Some(full_size) {
            let tex = Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, full_size)?;
            self.scene_tex = Some((full_size, tex));
        }
        if self.half_tex.as_ref().map(|(s, _)| *s) != Some(half_size) {
            let tex = Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, half_size)?;
            self.half_tex = Some((half_size, tex));
        }
        if self.quarter_tex.as_ref().map(|(s, _)| *s) != Some(quarter_size) {
            let tex =
                Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, quarter_size)?;
            self.quarter_tex = Some((quarter_size, tex));
        }
        Ok(())
    }

    /// Runs a 2-pass Dual-Kawase downsample (`full -> 1/2 -> 1/4`) on the background elements
    /// and returns `TextureShaderElement` backdrops for each translucent `glass_regions` rectangle.
    #[allow(clippy::too_many_arguments)]
    pub fn generate_glass_backdrops<E>(
        &mut self,
        renderer: &mut GlowRenderer,
        bg_elements: &[E],
        front_to_back: bool,
        output_phys_size: Size<i32, Physical>,
        scale: f64,
        bg_color: Color32F,
        glass_regions: &[GlassRegion],
    ) -> Vec<TextureShaderElement>
    where
        E: RenderElement<GlowRenderer>,
    {
        if glass_regions.is_empty() {
            return Vec::new();
        }
        let out_w = output_phys_size.w.max(4);
        let out_h = output_phys_size.h.max(4);
        if self.ensure_textures(renderer, out_w, out_h).is_err() {
            return Vec::new();
        }
        self.commit.increment();

        let (_, mut scene_tex) = self.scene_tex.take().unwrap();
        let (half_buf_size, mut half_tex) = self.half_tex.take().unwrap();
        let (quarter_buf_size, mut quarter_tex) = self.quarter_tex.take().unwrap();

        let pass_ok: Result<(), GlesError> = (|| {
            // 1. Render background/window scene into `scene_tex`
            {
                let mut target = Bind::<GlesTexture>::bind(renderer, &mut scene_tex)?;
                let full_dmg = [Rectangle::from_size(output_phys_size)];
                let mut frame =
                    renderer.render(&mut target, output_phys_size, Transform::Flipped180)?;
                frame.clear(bg_color, &full_dmg)?;
                if front_to_back {
                    for elem in bg_elements.iter().rev() {
                        let src = elem.src();
                        let dst = elem.geometry(scale.into());
                        let dmg = [Rectangle::from_size(dst.size)];
                        elem.draw(&mut frame, src, dst, &dmg, &[])?;
                    }
                } else {
                    for elem in bg_elements.iter() {
                        let src = elem.src();
                        let dst = elem.geometry(scale.into());
                        let dmg = [Rectangle::from_size(dst.size)];
                        elem.draw(&mut frame, src, dst, &dmg, &[])?;
                    }
                }
                let _ = frame.finish()?;
            }

            // 2. Downsample 1: `scene_tex` -> `half_tex`
            {
                let half_phys = Size::from((half_buf_size.w, half_buf_size.h));
                let half_logical = Rectangle::from_size(Size::from((half_buf_size.w, half_buf_size.h)));
                let elem = TextureShaderElement::new(
                    smithay::backend::renderer::element::texture::TextureRenderElement::from_static_texture(
                        smithay::backend::renderer::element::Id::new(),
                        renderer.context_id(),
                        (0.0, 0.0),
                        scene_tex.clone(),
                        1,
                        Transform::Normal,
                        Some(1.0),
                        None,
                        Some(half_logical.size),
                        None,
                        Kind::Unspecified,
                    ),
                    self.down_program.clone(),
                    vec![Uniform::new(
                        Cow::Borrowed("u_halfpixel"),
                        UniformValue::_2f(1.5 / (out_w as f32), 1.5 / (out_h as f32)),
                    )],
                );
                let mut target = Bind::<GlesTexture>::bind(renderer, &mut half_tex)?;
                let dmg = [Rectangle::from_size(half_phys)];
                let gles: &mut GlesRenderer = renderer.borrow_mut();
                let mut frame = gles.render(&mut target, half_phys, Transform::Flipped180)?;
                let src = elem.src();
                let dst = elem.geometry(1.0.into());
                <TextureShaderElement as RenderElement<GlesRenderer>>::draw(&elem, &mut frame, src, dst, &dmg, &[])?;
                let _ = frame.finish()?;
            }

            // 3. Downsample 2: `half_tex` -> `quarter_tex`
            {
                let q_phys = Size::from((quarter_buf_size.w, quarter_buf_size.h));
                let q_logical = Rectangle::from_size(Size::from((quarter_buf_size.w, quarter_buf_size.h)));
                let elem = TextureShaderElement::new(
                    smithay::backend::renderer::element::texture::TextureRenderElement::from_static_texture(
                        smithay::backend::renderer::element::Id::new(),
                        renderer.context_id(),
                        (0.0, 0.0),
                        half_tex.clone(),
                        1,
                        Transform::Normal,
                        Some(1.0),
                        None,
                        Some(q_logical.size),
                        None,
                        Kind::Unspecified,
                    ),
                    self.down_program.clone(),
                    vec![Uniform::new(
                        Cow::Borrowed("u_halfpixel"),
                        UniformValue::_2f(
                            1.5 / (half_buf_size.w.max(1) as f32),
                            1.5 / (half_buf_size.h.max(1) as f32),
                        ),
                    )],
                );
                let mut target = Bind::<GlesTexture>::bind(renderer, &mut quarter_tex)?;
                let dmg = [Rectangle::from_size(q_phys)];
                let gles: &mut GlesRenderer = renderer.borrow_mut();
                let mut frame = gles.render(&mut target, q_phys, Transform::Flipped180)?;
                let src = elem.src();
                let dst = elem.geometry(1.0.into());
                <TextureShaderElement as RenderElement<GlesRenderer>>::draw(&elem, &mut frame, src, dst, &dmg, &[])?;
                let _ = frame.finish()?;
            }
            Ok(())
        })();

        let mut backdrops = Vec::new();
        if pass_ok.is_ok() {
            let log_out_w = ((out_w as f64) / scale).round().max(1.0);
            let log_out_h = ((out_h as f64) / scale).round().max(1.0);
            let qw = quarter_buf_size.w.max(1) as f64;
            let qh = quarter_buf_size.h.max(1) as f64;

            for region in glass_regions {
                if region.rect.size.w <= 0 || region.rect.size.h <= 0 {
                    continue;
                }
                let sx = ((region.rect.loc.x as f64 / log_out_w) * qw).clamp(0.0, (qw - 1.0).max(0.0));
                let sy = ((region.rect.loc.y as f64 / log_out_h) * qh).clamp(0.0, (qh - 1.0).max(0.0));
                let sw = ((region.rect.size.w as f64 / log_out_w) * qw).clamp(1.0, (qw - sx).max(1.0));
                let sh = ((region.rect.size.h as f64 / log_out_h) * qh).clamp(1.0, (qh - sy).max(1.0));

                let phys_w = (region.rect.size.w as f64 * scale) as f32;
                let phys_h = (region.rect.size.h as f64 * scale) as f32;
                let phys_r = (region.corner_radius as f64 * scale) as f32;

                let elem = TextureShaderElement::new(
                    smithay::backend::renderer::element::texture::TextureRenderElement::from_static_texture(
                        smithay::backend::renderer::element::Id::new(),
                        renderer.context_id(),
                        ((region.rect.loc.x as f64) * scale, (region.rect.loc.y as f64) * scale),
                        quarter_tex.clone(),
                        1,
                        Transform::Normal,
                        Some(0.94),
                        Some(Rectangle::new(Point::from((sx, sy)), Size::from((sw, sh)))),
                        Some(region.rect.size),
                        None,
                        Kind::Unspecified,
                    ),
                    self.up_program.clone(),
                    vec![
                        Uniform::new(
                            Cow::Borrowed("u_halfpixel"),
                            UniformValue::_2f(1.5 / (qw as f32), 1.5 / (qh as f32)),
                        ),
                        Uniform::new(Cow::Borrowed("u_tint"), UniformValue::_1f(region.tint)),
                        Uniform::new(
                            Cow::Borrowed("u_region"),
                            UniformValue::_4f(sx as f32, sy as f32, sw as f32, sh as f32),
                        ),
                        Uniform::new(
                            Cow::Borrowed("u_size"),
                            UniformValue::_2f(phys_w, phys_h),
                        ),
                        Uniform::new(
                            Cow::Borrowed("u_radius"),
                            UniformValue::_1f(phys_r),
                        ),
                    ],
                );
                backdrops.push(elem);
            }
        }

        self.scene_tex = Some(((out_w, out_h).into(), scene_tex));
        self.half_tex = Some((half_buf_size, half_tex));
        self.quarter_tex = Some((quarter_buf_size, quarter_tex));
        backdrops
    }

    /// Renders `scene_elements` into the offscreen `scene_tex` and returns a full-screen
    /// `TextureShaderElement` that applies luma-preserving Kelvin color temperature grading
    /// (preserving deep dark blacks while attenuating blue light on midtones/highlights).
    pub fn generate_night_light_element<E>(
        &mut self,
        renderer: &mut GlowRenderer,
        scene_elements: &[E],
        front_to_back: bool,
        output_phys_size: Size<i32, Physical>,
        scale: f64,
        bg_color: Color32F,
    ) -> Option<TextureShaderElement>
    where
        E: RenderElement<GlowRenderer>,
    {
        let out_w = output_phys_size.w.max(4);
        let out_h = output_phys_size.h.max(4);
        if self.ensure_textures(renderer, out_w, out_h).is_err() {
            return None;
        }
        self.commit.increment();

        let (_, mut scene_tex) = self.scene_tex.take()?;
        let pass_ok: Result<(), GlesError> = (|| {
            let mut target = Bind::<GlesTexture>::bind(renderer, &mut scene_tex)?;
            let full_dmg = [Rectangle::from_size(output_phys_size)];
            let mut frame =
                renderer.render(&mut target, output_phys_size, Transform::Flipped180)?;
            frame.clear(bg_color, &full_dmg)?;
            if front_to_back {
                for elem in scene_elements.iter().rev() {
                    let src = elem.src();
                    let dst = elem.geometry(scale.into());
                    let dmg = [Rectangle::from_size(dst.size)];
                    elem.draw(&mut frame, src, dst, &dmg, &[])?;
                }
            } else {
                for elem in scene_elements.iter() {
                    let src = elem.src();
                    let dst = elem.geometry(scale.into());
                    let dmg = [Rectangle::from_size(dst.size)];
                    elem.draw(&mut frame, src, dst, &dmg, &[])?;
                }
            }
            let _ = frame.finish()?;
            Ok(())
        })();

        let log_out_w = ((out_w as f64) / scale).round().max(1.0) as i32;
        let log_out_h = ((out_h as f64) / scale).round().max(1.0) as i32;

        let result = if pass_ok.is_ok() {
            Some(TextureShaderElement::new(
                smithay::backend::renderer::element::texture::TextureRenderElement::from_static_texture(
                    smithay::backend::renderer::element::Id::new(),
                    renderer.context_id(),
                    (0.0, 0.0),
                    scene_tex.clone(),
                    1,
                    Transform::Normal,
                    Some(1.0),
                    None,
                    Some(Size::from((log_out_w, log_out_h))),
                    None,
                    Kind::Unspecified,
                ),
                self.night_light_program.clone(),
                vec![Uniform::new(
                    Cow::Borrowed("u_warmth"),
                    UniformValue::_1f(0.85),
                )],
            ))
        } else {
            None
        };

        self.scene_tex = Some(((out_w, out_h).into(), scene_tex));
        result
    }
}

pub const GLOW_SHADER_SRC: &str = r#"
precision mediump float;
varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

uniform vec4 u_glow_color;
uniform float u_radius;
uniform float u_glow_size;

// Signed distance function for a 2D rounded box
float sd_rounded_box(vec2 p, vec2 b, float r) {
    vec2 q = abs(p) - b + vec2(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - r;
}

void main() {
    // Current pixel coordinate in local element coordinates [0, size.x] x [0, size.y]
    vec2 pixel_coord = v_coords * size;
    vec2 center = size * 0.5;
    vec2 half_window = center - vec2(u_glow_size);

    vec2 p = pixel_coord - center;
    float dist = sd_rounded_box(p, half_window, u_radius);

    if (dist < 0.0) {
        // Inside the window bounds - discard so window surface shows through
        discard;
    } else if (dist <= u_glow_size) {
        // True smooth exponential Gaussian falloff
        float norm_d = dist / u_glow_size;
        // Soft exponential falloff for real blur simulation
        float intensity = exp(-norm_d * norm_d * 4.2);
        // Bright core highlight line near window border
        float core = exp(-norm_d * 12.0) * 0.45;
        float final_a = clamp((intensity + core) * u_glow_color.a * alpha, 0.0, 1.0);

        gl_FragColor = vec4(u_glow_color.rgb * final_a, final_a);
    } else {
        discard;
    }
}
"#;

pub struct GlowShaderManager {
    program: GlesPixelProgram,
}

impl GlowShaderManager {
    pub fn new(
        renderer: &mut GlesRenderer,
    ) -> Result<Self, smithay::backend::renderer::gles::GlesError> {
        let uniforms = [
            UniformName::new("u_glow_color", UniformType::_4f),
            UniformName::new("u_radius", UniformType::_1f),
            UniformName::new("u_glow_size", UniformType::_1f),
        ];

        let program = renderer.compile_custom_pixel_shader(GLOW_SHADER_SRC, &uniforms)?;
        Ok(Self { program })
    }

    pub fn create_element(
        &self,
        window_rect: Rectangle<i32, Logical>,
        glow_size: i32,
        corner_radius: f32,
        color: [f32; 4],
    ) -> PixelShaderElement {
        let glow_area = Rectangle::new(
            Point::from((window_rect.loc.x - glow_size, window_rect.loc.y - glow_size)),
            (
                window_rect.size.w + glow_size * 2,
                window_rect.size.h + glow_size * 2,
            )
                .into(),
        );

        let uniforms = vec![
            Uniform::new(
                Cow::Borrowed("u_glow_color"),
                UniformValue::_4f(color[0], color[1], color[2], color[3]),
            ),
            Uniform::new(Cow::Borrowed("u_radius"), UniformValue::_1f(corner_radius)),
            Uniform::new(
                Cow::Borrowed("u_glow_size"),
                UniformValue::_1f(glow_size as f32),
            ),
        ];

        PixelShaderElement::new(
            self.program.clone(),
            glow_area,
            None,
            1.0,
            uniforms,
            Kind::Unspecified,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glass_region_presets() {
        let rect = Rectangle::new(Point::from((100, 100)), Size::from((800, 600)));
        let win_glass = GlassRegion::window(rect);
        assert_eq!(win_glass.rect, rect);
        assert_eq!(win_glass.corner_radius, 8.0);
        assert!((win_glass.tint - 0.22).abs() < 0.01);

        let panel_glass = GlassRegion::panel(rect);
        assert_eq!(panel_glass.corner_radius, 16.0);
        assert!((panel_glass.tint - 0.32).abs() < 0.01);

        let from_rect: GlassRegion = rect.into();
        assert_eq!(from_rect, win_glass);
    }
}

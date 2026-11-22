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

// Continuous G2 Squircle (L4 superellipse) signed distance function
float squircle_box(vec2 p, vec2 b, float r) {
    vec2 d = max(abs(p) - b + vec2(r), 0.0);
    float l4 = pow(pow(d.x, 4.0) + pow(d.y, 4.0), 0.25);
    return min(max( abs(p.x) - b.x + r, abs(p.y) - b.y + r ), 0.0) + l4 - r;
}

void main() {
    float mask = 1.0;
    if (u_radius > 0.5 && u_region.z > 0.0001 && u_region.w > 0.0001) {
        vec2 norm_coord = (v_coords - u_region.xy) / u_region.zw;
        vec2 p = (norm_coord - 0.5) * u_size;
        vec2 b = u_size * 0.5;
        float dist = squircle_box(p, b, u_radius);
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
    // Slight chroma saturation boost (+10%) + procedural smoked acrylic micro-grain
    float luma = dot(blurred.rgb, vec3(0.2126, 0.7152, 0.0722));
    vec3 saturated = mix(vec3(luma), blurred.rgb, 1.10);
    float grain = (fract(sin(dot(v_coords * 512.0, vec2(12.9898, 78.233))) * 43758.5453) - 0.5) * 0.012;
    vec3 frosted = clamp(mix(saturated, vec3(0.09, 0.10, 0.12), u_tint) + vec3(grain), 0.0, 1.0);
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

    pub fn snap_preview(rect: Rectangle<i32, Logical>) -> Self {
        Self {
            rect,
            corner_radius: 12.0,
            tint: 0.16,
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
uniform vec4 u_border_color;
uniform float u_border_width;
uniform vec2 u_radii;
uniform float u_glow_size;

float sd_rounded_box_tb(vec2 p, vec2 b, vec2 radii) {
    float r = (p.y < 0.0) ? radii.x : radii.y;
    vec2 q = abs(p) - b + vec2(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - r;
}

void main() {
    vec2 pixel_coord = v_coords * size;
    vec2 center = size * 0.5;
    vec2 half_window = center - vec2(u_glow_size);

    vec2 p = pixel_coord - center;
    float dist = sd_rounded_box_tb(p, half_window, u_radii);

    if (dist < -0.5) {
        discard;
    }

    float d_pos = max(dist, 0.0);
    // Smooth fade to zero before the quad boundary to prevent any hard edge cutoff
    float edge_fade = 1.0 - smoothstep(u_glow_size * 0.68, u_glow_size, d_pos);

    // Layer 1: Tight contact ambient occlusion (+1.5px Y)
    vec2 p_ao = p - vec2(0.0, 1.5);
    float dist_ao = max(sd_rounded_box_tb(p_ao, half_window, u_radii + vec2(1.0)), 0.0);
    float ao_a = exp(-dist_ao * 0.26) * 0.24 * edge_fade;

    // Layer 2: Soft downward-cast Gaussian elevation shadow (+8px Y)
    vec2 p_key = p - vec2(0.0, 8.0);
    float dist_key = max(sd_rounded_box_tb(p_key, half_window, u_radii + vec2(4.0)), 0.0);
    float shadow_sigma = max(u_glow_size * 0.36, 5.0);
    float key_a = exp(-(dist_key * dist_key) / (2.0 * shadow_sigma * shadow_sigma)) * 0.36 * edge_fade;

    float shadow_a = clamp(ao_a + key_a, 0.0, 0.52);

    // Layer 3: Optional subtle halo (0.0 for normal windows; used only by snap preview / high-vis)
    float halo_sigma = max(u_glow_size * 0.24, 3.0);
    float halo_a = (u_glow_color.a > 0.001)
        ? exp(-(d_pos * d_pos) / (2.0 * halo_sigma * halo_sigma)) * u_glow_color.a * edge_fade
        : 0.0;

    // Layer 4: Crisp anti-aliased hairline glass rim
    float border_mask = (1.0 - smoothstep(u_border_width - 0.60, u_border_width + 0.40, dist))
                      * smoothstep(-0.5, 0.15, dist);
    float border_a = border_mask * u_border_color.a;

    // Premultiplied Porter-Duff src-over compositing: shadow -> halo -> hairline rim
    vec3 col = vec3(0.0);
    float out_a = shadow_a;

    if (halo_a > 0.001) {
        vec3 halo_pm = u_glow_color.rgb * halo_a;
        col = halo_pm + col * (1.0 - halo_a);
        out_a = halo_a + out_a * (1.0 - halo_a);
    }

    if (border_a > 0.001) {
        vec3 border_pm = u_border_color.rgb * border_a;
        col = border_pm + col * (1.0 - border_a);
        out_a = border_a + out_a * (1.0 - border_a);
    }

    if (out_a <= 0.001) {
        discard;
    }

    gl_FragColor = vec4(col, out_a) * alpha;
}
"#;

pub const ROUNDED_HEADER_SHADER_SRC: &str = r#"
precision mediump float;
varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

uniform vec4 u_bg_top;
uniform vec4 u_bg_bottom;
uniform vec4 u_rim_color;
uniform float u_radius;

float sd_top_rounded(vec2 p, vec2 b, float r) {
    float cr = (p.y < 0.0) ? r : 0.0;
    vec2 q = abs(p) - b + vec2(cr);
    return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - cr;
}

void main() {
    vec2 pixel_coord = v_coords * size;
    vec2 half_size = size * 0.5;
    vec2 p = pixel_coord - half_size;

    float dist = sd_top_rounded(p, half_size, u_radius);
    float mask = 1.0 - smoothstep(-0.6, 0.5, dist);
    if (mask <= 0.001) {
        discard;
    }

    float t = clamp(v_coords.y, 0.0, 1.0);
    vec4 bg = mix(u_bg_top, u_bg_bottom, t);

    // Subtle 1px curved top glass highlight
    float inner_rim = smoothstep(-1.8, -0.4, dist) * (1.0 - smoothstep(-0.4, 0.4, dist));
    float top_weight = smoothstep(0.45, 0.0, t);
    vec3 rgb = mix(bg.rgb, u_rim_color.rgb, inner_rim * top_weight * u_rim_color.a);

    float out_a = bg.a * mask * alpha;
    gl_FragColor = vec4(rgb * out_a, out_a);
}
"#;

pub struct GlowShaderManager {
    program: GlesPixelProgram,
    header_program: GlesPixelProgram,
}

impl GlowShaderManager {
    pub fn new(
        renderer: &mut GlesRenderer,
    ) -> Result<Self, smithay::backend::renderer::gles::GlesError> {
        let uniforms = [
            UniformName::new("u_glow_color", UniformType::_4f),
            UniformName::new("u_border_color", UniformType::_4f),
            UniformName::new("u_border_width", UniformType::_1f),
            UniformName::new("u_radii", UniformType::_2f),
            UniformName::new("u_glow_size", UniformType::_1f),
        ];
        let header_uniforms = [
            UniformName::new("u_bg_top", UniformType::_4f),
            UniformName::new("u_bg_bottom", UniformType::_4f),
            UniformName::new("u_rim_color", UniformType::_4f),
            UniformName::new("u_radius", UniformType::_1f),
        ];

        let program = renderer.compile_custom_pixel_shader(GLOW_SHADER_SRC, &uniforms)?;
        let header_program =
            renderer.compile_custom_pixel_shader(ROUNDED_HEADER_SHADER_SRC, &header_uniforms)?;
        Ok(Self {
            program,
            header_program,
        })
    }

    pub fn create_styled_element(
        &self,
        window_rect: Rectangle<i32, Logical>,
        glow_size: i32,
        corner_radii: (f32, f32),
        glow_color: [f32; 4],
        border_color: [f32; 4],
        border_width: f32,
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
                UniformValue::_4f(glow_color[0], glow_color[1], glow_color[2], glow_color[3]),
            ),
            Uniform::new(
                Cow::Borrowed("u_border_color"),
                UniformValue::_4f(
                    border_color[0],
                    border_color[1],
                    border_color[2],
                    border_color[3],
                ),
            ),
            Uniform::new(
                Cow::Borrowed("u_border_width"),
                UniformValue::_1f(border_width),
            ),
            Uniform::new(
                Cow::Borrowed("u_radii"),
                UniformValue::_2f(corner_radii.0, corner_radii.1),
            ),
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

    /// Creates a single-pass rounded top-corners SSD titlebar plate.
    pub fn create_rounded_header_element(
        &self,
        header_rect: Rectangle<i32, Logical>,
        top_radius: f32,
        bg_top: [f32; 4],
        bg_bottom: [f32; 4],
        rim_color: [f32; 4],
    ) -> PixelShaderElement {
        let uniforms = vec![
            Uniform::new(
                Cow::Borrowed("u_bg_top"),
                UniformValue::_4f(bg_top[0], bg_top[1], bg_top[2], bg_top[3]),
            ),
            Uniform::new(
                Cow::Borrowed("u_bg_bottom"),
                UniformValue::_4f(bg_bottom[0], bg_bottom[1], bg_bottom[2], bg_bottom[3]),
            ),
            Uniform::new(
                Cow::Borrowed("u_rim_color"),
                UniformValue::_4f(rim_color[0], rim_color[1], rim_color[2], rim_color[3]),
            ),
            Uniform::new(Cow::Borrowed("u_radius"), UniformValue::_1f(top_radius)),
        ];

        PixelShaderElement::new(
            self.header_program.clone(),
            header_rect,
            None,
            1.0,
            uniforms,
            Kind::Unspecified,
        )
    }

    /// Creates a clean, grounded elevation shadow + subtle 1px glass rim for inactive windows.
    pub fn create_inactive_shadow_element(
        &self,
        window_rect: Rectangle<i32, Logical>,
        corner_radii: (f32, f32),
        border_color: [f32; 4],
    ) -> PixelShaderElement {
        self.create_styled_element(
            window_rect,
            28,
            corner_radii,
            [0.0, 0.0, 0.0, 0.0],
            border_color,
            1.0,
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

//! `wlr-screencopy-unstable-v1` protocol implementation and native GPU framebuffer screenshot exporter.

use std::io::BufWriter;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::RenderElement;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::{Bind, ExportMem, Frame, Offscreen, Renderer};
use smithay::output::Output;
use smithay::reexports::wayland_protocols_wlr::screencopy::v1::server::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
};
use smithay::reexports::wayland_server::{
    backend::GlobalId,
    protocol::{wl_buffer::WlBuffer, wl_output::WlOutput, wl_shm},
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::utils::{Buffer as BufferCoord, Logical, Physical, Rectangle, Size, Transform};
use smithay::wayland::shm::with_buffer_contents_mut;
use tracing::{error, info, warn};

use crate::state::CortenCompositor;

const VERSION: u32 = 3;

/// Global state for `zwlr_screencopy_manager_v1`.
pub struct ScreencopyManagerState {
    global: GlobalId,
}

impl ScreencopyManagerState {
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwlrScreencopyManagerV1, ()>
            + Dispatch<ZwlrScreencopyManagerV1, ()>
            + Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameState>
            + 'static,
    {
        let global = display.create_global::<D, ZwlrScreencopyManagerV1, ()>(VERSION, ());
        Self { global }
    }

    #[allow(dead_code)]
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

/// User data attached to each `zwlr_screencopy_frame_v1` resource.
#[derive(Debug, Clone)]
pub struct ScreencopyFrameState {
    pub rect: Rectangle<i32, BufferCoord>,
    pub overlay_cursor: bool,
}

/// A queued client screencopy frame waiting for the next compositor render pass.
#[derive(Debug, Clone)]
pub struct PendingScreencopy {
    pub frame: ZwlrScreencopyFrameV1,
    pub buffer: WlBuffer,
    pub rect: Rectangle<i32, BufferCoord>,
    pub with_damage: bool,
}

/// A queued native compositor screenshot (triggered by `PrintScreen`, `Super+Shift+S`, or IPC).
#[derive(Debug, Clone)]
pub struct PendingScreenshot {
    /// Optional logical capture region; `None` captures the entire output.
    pub region: Option<Rectangle<i32, Logical>>,
}

/// Clamp a requested logical region against the output's physical dimensions and scale.
pub fn clamp_capture_region(
    output_phys_size: Size<i32, Physical>,
    scale: f64,
    logical_region: Option<(i32, i32, i32, i32)>,
) -> Rectangle<i32, BufferCoord> {
    let out_w = output_phys_size.w.max(1);
    let out_h = output_phys_size.h.max(1);

    if let Some((lx, ly, lw, lh)) = logical_region {
        let px = ((lx as f64) * scale).round() as i32;
        let py = ((ly as f64) * scale).round() as i32;
        let pw = ((lw.max(1) as f64) * scale).round() as i32;
        let ph = ((lh.max(1) as f64) * scale).round() as i32;

        let x0 = px.clamp(0, (out_w - 1).max(0));
        let y0 = py.clamp(0, (out_h - 1).max(0));
        let w = pw.clamp(1, (out_w - x0).max(1));
        let h = ph.clamp(1, (out_h - y0).max(1));
        Rectangle::new((x0, y0).into(), (w, h).into())
    } else {
        Rectangle::from_size((out_w, out_h).into())
    }
}

fn output_phys_and_scale(state: &CortenCompositor, wl_output: &WlOutput) -> (Size<i32, Physical>, f64) {
    if let Some(output) = Output::from_resource(wl_output) {
        let size = output
            .current_mode()
            .map(|m| m.size)
            .unwrap_or_else(|| Size::from((state.output_size.w, state.output_size.h)));
        let scale = output.current_scale().fractional_scale();
        (size, scale)
    } else {
        (
            Size::from((state.output_size.w.max(1), state.output_size.h.max(1))),
            1.0,
        )
    }
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for CortenCompositor {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for CortenCompositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput {
                frame,
                overlay_cursor,
                output,
            } => {
                let (phys_size, scale) = output_phys_and_scale(state, &output);
                let rect = clamp_capture_region(phys_size, scale, None);
                let frame_state = ScreencopyFrameState {
                    rect,
                    overlay_cursor: overlay_cursor != 0,
                };
                let frame_obj = data_init.init(frame, frame_state);
                send_frame_buffer_info(&frame_obj, rect);
            }
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion {
                frame,
                overlay_cursor,
                output,
                x,
                y,
                width,
                height,
            } => {
                let (phys_size, scale) = output_phys_and_scale(state, &output);
                let rect = clamp_capture_region(phys_size, scale, Some((x, y, width, height)));
                let frame_state = ScreencopyFrameState {
                    rect,
                    overlay_cursor: overlay_cursor != 0,
                };
                let frame_obj = data_init.init(frame, frame_state);
                send_frame_buffer_info(&frame_obj, rect);
            }
            zwlr_screencopy_manager_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

fn send_frame_buffer_info(frame: &ZwlrScreencopyFrameV1, rect: Rectangle<i32, BufferCoord>) {
    let width = rect.size.w.max(1) as u32;
    let height = rect.size.h.max(1) as u32;
    let stride = width * 4;
    frame.buffer(wl_shm::Format::Argb8888, width, height, stride);
    if frame.version() >= 3 {
        frame.buffer_done();
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameState> for CortenCompositor {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &ScreencopyFrameState,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => {
                let _ = data.overlay_cursor;
                state.pending_screencopies.push(PendingScreencopy {
                    frame: resource.clone(),
                    buffer,
                    rect: data.rect,
                    with_damage: false,
                });
            }
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => {
                state.pending_screencopies.push(PendingScreencopy {
                    frame: resource.clone(),
                    buffer,
                    rect: data.rect,
                    with_damage: true,
                });
            }
            zwlr_screencopy_frame_v1::Request::Destroy => {
                state
                    .pending_screencopies
                    .retain(|pending| pending.frame != *resource);
            }
            _ => {}
        }
    }
}

/// Encode raw RGBA8888 bytes into a PNG byte vector.
pub fn encode_rgba_to_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| "overflow computing RGBA buffer size".to_string())?;
    if rgba.len() < expected {
        return Err(format!(
            "RGBA buffer too small: got {} bytes, expected at least {}",
            rgba.len(),
            expected
        ));
    }

    let mut out = Vec::with_capacity(expected / 2);
    {
        let writer = BufWriter::new(&mut out);
        let mut encoder = png::Encoder::new(writer, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut png_writer = encoder
            .write_header()
            .map_err(|e| format!("failed to write PNG header: {e}"))?;
        png_writer
            .write_image_data(&rgba[..expected])
            .map_err(|e| format!("failed to write PNG data: {e}"))?;
    }
    Ok(out)
}

/// Default destination path (`~/Pictures/Screenshot_<unix_ms>.png`).
pub fn next_screenshot_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let pictures = PathBuf::from(home).join("Pictures");
    let _ = std::fs::create_dir_all(&pictures);
    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    pictures.join(format!("Screenshot_{ts_ms}.png"))
}

/// Render all scene elements to an offscreen `GlesTexture` and fulfill any pending
/// `wlr-screencopy` client frames and native compositor screenshots.
pub fn fulfill_pending_captures<E>(
    state: &mut CortenCompositor,
    renderer: &mut GlowRenderer,
    elements: &[E],
    front_to_back: bool,
    output_size: Size<i32, Physical>,
    scale: f64,
    bg_color: smithay::backend::renderer::Color32F,
) where
    E: RenderElement<GlowRenderer>,
{
    if state.pending_screencopies.is_empty() && state.pending_screenshot.is_none() {
        return;
    }

    let pending_copies = std::mem::take(&mut state.pending_screencopies);
    let pending_shot = state.pending_screenshot.take();

    let out_w = output_size.w.max(1);
    let out_h = output_size.h.max(1);
    let buffer_size: Size<i32, BufferCoord> = (out_w, out_h).into();

    let mut offscreen_tex: GlesTexture =
        match Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, buffer_size) {
            Ok(tex) => tex,
            Err(err) => {
                error!("screencopy: failed to allocate offscreen texture: {err:?}");
                for req in pending_copies {
                    req.frame.failed();
                }
                return;
            }
        };

    // Render scene into `offscreen_tex` with `Transform::Flipped180` so OpenGL's
    // lower-left origin matches Wayland/PNG top-left row order directly.
    let render_res: Result<(), smithay::backend::renderer::gles::GlesError> = (|| {
        let mut target = Bind::<GlesTexture>::bind(renderer, &mut offscreen_tex)?;
        let full_damage = [Rectangle::from_size(output_size)];
        let mut frame = renderer.render(&mut target, output_size, Transform::Flipped180)?;
        frame.clear(bg_color, &full_damage)?;

        if front_to_back {
            for elem in elements.iter().rev() {
                let src = elem.src();
                let dst = elem.geometry(scale.into());
                let dmg = [Rectangle::from_size(dst.size)];
                elem.draw(&mut frame, src, dst, &dmg, &[])?;
            }
        } else {
            for elem in elements.iter() {
                let src = elem.src();
                let dst = elem.geometry(scale.into());
                let dmg = [Rectangle::from_size(dst.size)];
                elem.draw(&mut frame, src, dst, &dmg, &[])?;
            }
        }
        let _ = frame.finish()?;
        Ok(())
    })();

    if let Err(err) = render_res {
        error!("screencopy: offscreen render failed: {err:?}");
        for req in pending_copies {
            req.frame.failed();
        }
        return;
    }

    // 1. Fulfill `wlr-screencopy-unstable-v1` client requests
    for req in pending_copies {
        let rect = clamp_capture_region(
            output_size,
            1.0,
            Some((
                req.rect.loc.x,
                req.rect.loc.y,
                req.rect.size.w,
                req.rect.size.h,
            )),
        );
        let mapping = match renderer.copy_texture(&offscreen_tex, rect, Fourcc::Argb8888) {
            Ok(m) => m,
            Err(err) => {
                warn!("screencopy: copy_texture failed: {err:?}");
                req.frame.failed();
                continue;
            }
        };
        let src_bytes = match renderer.map_texture(&mapping) {
            Ok(b) => b,
            Err(err) => {
                warn!("screencopy: map_texture failed: {err:?}");
                req.frame.failed();
                continue;
            }
        };

        let width = rect.size.w.max(1) as usize;
        let height = rect.size.h.max(1) as usize;
        let src_stride = width * 4;

        let copy_ok = with_buffer_contents_mut(&req.buffer, |dst_ptr, dst_len, buf_data| {
            let dst_stride = buf_data.stride.max(0) as usize;
            let dst_h = buf_data.height.max(0) as usize;
            let dst_w = buf_data.width.max(0) as usize;
            if dst_w < width
                || dst_h < height
                || dst_stride < src_stride
                || dst_len < dst_stride.saturating_mul(height)
            {
                return false;
            }
            // SAFETY: `with_buffer_contents_mut` provides a valid mutable SHM slice of `dst_len` bytes.
            let dst_slice = unsafe { std::slice::from_raw_parts_mut(dst_ptr, dst_len) };
            for row in 0..height {
                let s_off = row * src_stride;
                let d_off = row * dst_stride;
                if s_off + src_stride <= src_bytes.len() && d_off + src_stride <= dst_slice.len() {
                    dst_slice[d_off..d_off + src_stride]
                        .copy_from_slice(&src_bytes[s_off..s_off + src_stride]);
                }
            }
            true
        });

        match copy_ok {
            Ok(true) => {
                if req.with_damage && req.frame.version() >= 2 {
                    req.frame.damage(0, 0, width as u32, height as u32);
                }
                req.frame.flags(zwlr_screencopy_frame_v1::Flags::empty());
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default();
                let secs = now.as_secs();
                let tv_sec_hi = (secs >> 32) as u32;
                let tv_sec_lo = (secs & 0xFFFF_FFFF) as u32;
                req.frame.ready(tv_sec_hi, tv_sec_lo, now.subsec_nanos());
            }
            _ => {
                warn!("screencopy: client SHM buffer incompatible with frame geometry");
                req.frame.failed();
            }
        }
    }

    // 2. Fulfill native compositor screenshot (`PendingScreenshot`)
    if let Some(shot) = pending_shot {
        let rect = clamp_capture_region(
            output_size,
            scale,
            shot.region
                .map(|r| (r.loc.x, r.loc.y, r.size.w, r.size.h)),
        );
        match renderer.copy_texture(&offscreen_tex, rect, Fourcc::Abgr8888) {
            Ok(mapping) => match renderer.map_texture(&mapping) {
                Ok(bytes) => {
                    let rgba = bytes.to_vec();
                    let w = rect.size.w.max(1) as u32;
                    let h = rect.size.h.max(1) as u32;
                    std::thread::spawn(move || {
                        match encode_rgba_to_png(w, h, &rgba) {
                            Ok(png_bytes) => {
                                let path = next_screenshot_path();
                                if let Err(err) = std::fs::write(&path, &png_bytes) {
                                    error!("failed to save screenshot to {}: {err}", path.display());
                                } else {
                                    info!("Saved native compositor screenshot to {}", path.display());
                                }
                                // Also pipe PNG into `wl-copy --type image/png` if available.
                                if let Ok(mut child) = Command::new("wl-copy")
                                    .args(["--type", "image/png"])
                                    .stdin(Stdio::piped())
                                    .stdout(Stdio::null())
                                    .stderr(Stdio::null())
                                    .spawn()
                                {
                                    if let Some(mut stdin) = child.stdin.take() {
                                        let _ = std::io::Write::write_all(&mut stdin, &png_bytes);
                                    }
                                    let _ = child.wait();
                                }
                            }
                            Err(err) => {
                                error!("failed to encode screenshot PNG: {err}");
                            }
                        }
                    });
                }
                Err(err) => error!("native screenshot map_texture failed: {err:?}"),
            },
            Err(err) => error!("native screenshot copy_texture failed: {err:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_region_stays_within_output_bounds() {
        let phys = Size::from((1920, 1080));
        let full = clamp_capture_region(phys, 1.0, None);
        assert_eq!(full.loc.x, 0);
        assert_eq!(full.loc.y, 0);
        assert_eq!(full.size.w, 1920);
        assert_eq!(full.size.h, 1080);

        let clamped = clamp_capture_region(phys, 1.5, Some((1000, 600, 500, 400)));
        assert_eq!(clamped.loc.x, 1500);
        assert_eq!(clamped.loc.y, 900);
        assert_eq!(clamped.size.w, 420);
        assert_eq!(clamped.size.h, 180);
    }

    #[test]
    fn rgba_png_encoder_produces_valid_png_signature() {
        let rgba = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let png = encode_rgba_to_png(2, 2, &rgba).expect("encode 2x2 RGBA PNG");
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]));
    }
}


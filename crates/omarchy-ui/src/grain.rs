//! Grain — a fine, fixed noise laid over a surface so it reads as frosted.
//!
//! gpui cannot blur what it has painted, so a translucent bar over scrolling
//! rows is a veil: the rows show through it, sharp. A grain over the veil is
//! the nearest thing to frost the primitives allow — it breaks the rows'
//! edges up and pulls the eye to the surface rather than through it — and
//! it is also what gives the pointer's glow ([`crate::RevealHighlight`]) a
//! texture rather than a flat gradient.
//!
//! One noise tile, built once per palette and tiled from the **window's
//! origin**, never the element's: a bar that resizes or a glow that follows
//! the pointer moves across the grain rather than dragging it along, and two
//! bars side by side share one continuous surface. The tile is sized in
//! device pixels, so a speck is one physical pixel at any scale factor.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Bounds, ContentMask, Corners, Element, ElementId, Global, GlobalElementId,
    Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, RenderImage, Window, fill, point, px,
    size,
};

use crate::{ActiveTheme as _, Theme};

/// The grain's look: what colour the specks are and how strong.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GrainStyle {
    /// The lighter half of the specks.
    pub light: Hsla,
    /// The darker half.
    pub dark: Hsla,
    /// The specks' peak alpha; each speck draws a random fraction of it.
    pub strength: f32,
}

impl GrainStyle {
    /// Frost for a veil: specks in the foreground and the sunken ground,
    /// at the edge of perception — felt as a texture, not seen as noise,
    /// and never competing with the bar's own text.
    pub fn frost(theme: &Theme) -> Self {
        Self {
            light: theme.foreground(),
            dark: theme.sunken(),
            strength: 0.04,
        }
    }
}

/// Tile the grain over `clip`, from the window origin, clipped to `clip`
/// with `radii` — a circle when `radii` is half of a square `clip`.
///
/// `alpha` is the tile's peak alpha, applied when the tile is built; pass
/// one tile's worth and paint again to stack, as the glow does.
pub fn paint_grain(
    clip: Bounds<Pixels>,
    radii: Corners<Pixels>,
    style: GrainStyle,
    alpha: f32,
    window: &mut Window,
    cx: &mut App,
) {
    if alpha <= 0.0 || clip.size.width <= px(0.) || clip.size.height <= px(0.) {
        return;
    }
    let tile = cx.default_global::<NoiseTiles>().get(style, alpha);
    // The tile is `TILE` device pixels; place it at that many *logical*
    // pixels over the scale factor so its specks land on physical pixels.
    let step = TILE as f32 / window.scale_factor();
    let first_col = (f32::from(clip.left()) / step).floor() as i32;
    let last_col = (f32::from(clip.right()) / step).ceil() as i32;
    let first_row = (f32::from(clip.top()) / step).floor() as i32;
    let last_row = (f32::from(clip.bottom()) / step).ceil() as i32;
    for row in first_row..last_row {
        for col in first_col..last_col {
            let image_bounds = Bounds {
                origin: point(px(col as f32 * step), px(row as f32 * step)),
                size: size(px(step), px(step)),
            };
            let _ = window.paint_image(clip, image_bounds, radii, tile.clone(), 0, false);
        }
    }
}

/// A surface with a fill and a grain over it, under its child: the veil a
/// bar draws over the rows scrolling beneath it.
///
/// ```ignore
/// Frosted::new(bar).fill(theme.bar_veil())
/// ```
pub struct Frosted {
    child: Option<AnyElement>,
    fill: Option<Hsla>,
    radius: f32,
    style: Option<GrainStyle>,
}

impl Frosted {
    pub fn new(child: impl IntoElement) -> Self {
        Self {
            child: Some(child.into_any_element()),
            fill: None,
            radius: 0.0,
            style: None,
        }
    }

    /// The colour under the grain. Without one the grain lies straight on
    /// whatever is beneath.
    pub fn fill(mut self, fill: Hsla) -> Self {
        self.fill = Some(fill);
        self
    }

    /// The corner radius of the fill and the grain's clip.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// Override [`GrainStyle::frost`].
    pub fn style(mut self, style: GrainStyle) -> Self {
        self.style = Some(style);
        self
    }
}

impl IntoElement for Frosted {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Frosted {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut child = self.child.take().expect("a frosted surface lays out once");
        let layout = child.request_layout(window, cx);
        (layout, child)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        child: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let style = self.style.unwrap_or_else(|| GrainStyle::frost(cx.theme()));
        let radii = Corners::all(px(self.radius));
        if let Some(color) = self.fill {
            let mut quad = fill(bounds, color);
            quad.corner_radii = radii;
            window.paint_quad(quad);
        }
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            paint_grain(bounds, radii, style, style.strength, window, cx);
        });
        child.paint(window, cx);
    }
}

/// Side of the noise tile, in device pixels.
const TILE: u32 = 128;

/// One tile per palette and alpha, built on first use.
#[derive(Default)]
struct NoiseTiles(HashMap<[u8; 7], Arc<RenderImage>>);

impl Global for NoiseTiles {}

impl NoiseTiles {
    fn get(&mut self, style: GrainStyle, alpha: f32) -> Arc<RenderImage> {
        let key = noise_key(style, alpha);
        self.0
            .entry(key)
            .or_insert_with(|| {
                let pixels = noise_pixels(TILE, key);
                let buffer = image::RgbaImage::from_raw(TILE, TILE, pixels)
                    .expect("noise buffer is exactly width * height * 4 bytes");
                Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
            })
            .clone()
    }
}

/// Light BGR, dark BGR, alpha — each quantised to a byte.
fn noise_key(style: GrainStyle, alpha: f32) -> [u8; 7] {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let bgr = |c: Hsla| {
        let rgba: gpui::Rgba = c.into();
        [byte(rgba.b), byte(rgba.g), byte(rgba.r)]
    };
    let [lb, lg, lr] = bgr(style.light);
    let [db, dg, dr] = bgr(style.dark);
    [lb, lg, lr, db, dg, dr, byte(alpha)]
}

/// A tile of specks: each pixel is the light or the dark colour, chosen at
/// random, at a random fraction of `key[6]`. Straight alpha, BGRA. Seeded
/// with a constant, so every tile of a palette is the same tile and the
/// grain never flickers between frames or builds.
fn noise_pixels(side: u32, key: [u8; 7]) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((side * side * 4) as usize);
    let mut state: u32 = 0x9E37_79B9;
    let mut next = || {
        // xorshift32: plenty for specks, and no dependency.
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    for _ in 0..side * side {
        let tone = next();
        let alpha = (next() >> 24) * key[6] as u32 / 255;
        let colour = if tone & 1 == 0 {
            &key[3..6]
        } else {
            &key[0..3]
        };
        pixels.extend_from_slice(colour);
        pixels.push(alpha as u8);
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_noise_tile_is_deterministic_two_toned_and_bounded() {
        let key = [1, 2, 3, 7, 8, 9, 100];
        let a = noise_pixels(16, key);
        let b = noise_pixels(16, key);
        assert_eq!(a, b, "the same key must give the same tile");
        assert_eq!(a.len(), 16 * 16 * 4);

        let (mut light, mut dark) = (0, 0);
        for pixel in a.chunks_exact(4) {
            match &pixel[..3] {
                [1, 2, 3] => light += 1,
                [7, 8, 9] => dark += 1,
                other => panic!("a speck of neither colour: {other:?}"),
            }
            assert!(pixel[3] <= 100, "alpha {} over the peak", pixel[3]);
        }
        // Both tones present in fair measure — a one-toned tile is a tint,
        // not a grain.
        assert!(light > 64 && dark > 64, "light {light} dark {dark}");
    }

    #[test]
    fn a_zero_alpha_key_is_an_empty_tile() {
        let pixels = noise_pixels(8, [255, 255, 255, 0, 0, 0, 0]);
        assert!(pixels.chunks_exact(4).all(|p| p[3] == 0));
    }
}

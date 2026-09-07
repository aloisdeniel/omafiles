//! Pointer-lit surfaces — the "reveal" treatment of Microsoft's Fluent
//! design, rebuilt from what gpui can paint.
//!
//! Two wrappers, composable on any element through the [`Reveal`] trait:
//!
//! - [`RevealHighlight`] paints a soft radial glow under the element, centred
//!   on the pointer, while the element is hovered. For surfaces with no
//!   outline — rows, tabs, glyph buttons — where the wash alone is flat.
//! - [`RevealBorder`] paints the element's outline lit from the pointer,
//!   brightest where the pointer is nearest, and it does so while the pointer
//!   is merely *near* the element. That is what makes a bar of buttons read
//!   as one lit surface as the pointer travels across it.
//!
//! gpui has no radial gradient, no custom shaders and no way to sample what
//! was painted underneath, so both are assembled from the primitives it does
//! have (`GPUI-NOTES.md` §4). The highlight is a small pre-rendered glow
//! texture, painted through `paint_image` with the element's own corner
//! radii as the clip. The border is twelve quads: two linear-gradient
//! segments per edge, split at the pointer's projection so each edge peaks
//! there, plus one solid arc per rounded corner.
//!
//! The highlight is grained as well — see [`crate::grain`] — so it reads as
//! a lit texture rather than a flat gradient.
//!
//! Repaints are driven from here. gpui only redraws on a hover *change*, and
//! a glow that follows the pointer has to redraw on every move — so each
//! wrapper registers a mouse-move listener and refreshes the window while
//! the pointer is inside its zone, or was on the previous frame (so the last
//! frame clears the light).

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Background, BorderStyle, Bounds, ContentMask, Corners, DispatchPhase, Edges,
    Element, ElementId, Global, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId,
    IntoElement, LayoutId, MouseMoveEvent, Pixels, Point, RenderImage, Window, fill,
    linear_color_stop, linear_gradient, point, px, quad, size, transparent_black,
};

use crate::{ActiveTheme as _, GrainStyle, Theme, grain::paint_grain};

/// Wrap any element in a reveal effect.
///
/// ```ignore
/// Button::new("ok", "OK").reveal_highlight().reveal_border()
/// ```
///
/// Order is paint order: the highlight goes under the element, so it is
/// applied first; the border goes over it, so it is applied last.
pub trait Reveal: IntoElement + Sized {
    /// A pointer-centred glow under the element while it is hovered.
    fn reveal_highlight(self) -> RevealHighlight {
        RevealHighlight::new(self)
    }

    /// The element's outline, lit from the pointer.
    fn reveal_border(self) -> RevealBorder {
        RevealBorder::new(self)
    }

    /// A container's outline, lit from the pointer across its whole
    /// length: the reach is the container's own size, so the edge nearest
    /// the pointer is bright and the far edge has faded, wherever the
    /// pointer is over the card. For modals and menus.
    fn reveal_frame(self) -> RevealBorder {
        RevealBorder::new(self).frame()
    }
}

impl<E: IntoElement> Reveal for E {}

/// The tuning both wrappers read when a builder has not overridden it.
///
/// Derived from the theme so it scales with `omarchy display text size`
/// like everything else: a bigger UI has a wider light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RevealStyle {
    /// The light's colour at full strength. Defaults to the foreground, so
    /// it brightens a dark theme and darkens a light one — what "lit" means
    /// relative to each ground.
    pub color: Hsla,
    /// How far from the pointer the light still shows, in logical pixels.
    pub reach: f32,
    /// Peak alpha, at the pointer.
    pub strength: f32,
    /// The grain's peak alpha, at the pointer — see [`crate::GrainStyle`].
    /// Zero for a smooth light. Only the highlight has a surface to grain.
    pub grain: f32,
}

impl RevealStyle {
    /// The highlight's defaults: a wide, faint glow. Faint because it sits
    /// under the hover wash and the two add up at the pointer.
    pub fn highlight(theme: &Theme) -> Self {
        Self {
            color: theme.foreground(),
            reach: theme.space().control_height() * 3.0,
            strength: 0.06,
            grain: 0.03,
        }
    }

    /// A frame's defaults: the reach is replaced by the container's size
    /// at paint, so only the colour and the peak matter here — softer than
    /// a button's, since a card's edge is long and the light spans it.
    pub fn frame(theme: &Theme) -> Self {
        Self {
            color: theme.foreground(),
            reach: 0.0,
            strength: 0.6,
            grain: 0.0,
        }
    }

    /// The border's defaults: a longer reach, since it lights neighbours,
    /// and a stronger peak, since a hairline needs it to be seen at all.
    pub fn border(theme: &Theme) -> Self {
        Self {
            color: theme.foreground(),
            reach: theme.space().control_height() * 4.0,
            strength: 0.55,
            grain: 0.0,
        }
    }
}

/// How bright the light is `distance` away from the pointer: `strength` at
/// the pointer, zero at `reach`, with a smoothstep between so neither end
/// shows a crease.
pub fn falloff(distance: f32, reach: f32, strength: f32) -> f32 {
    if reach <= 0.0 || distance >= reach {
        return 0.0;
    }
    let d = (distance / reach).max(0.0);
    let eased = 1.0 - (3.0 * d * d - 2.0 * d * d * d);
    strength * eased
}

/// `light` composited over `base`, alpha included.
///
/// Not gpui's `Hsla::blend`: that mixes the colour channels but keeps the
/// *base's* alpha, which is right over an opaque ground and wrong for a
/// translucent border — the light would change its hue and never its
/// strength.
fn over(light: Hsla, base: Hsla) -> Hsla {
    let (l, b): (gpui::Rgba, gpui::Rgba) = (light.into(), base.into());
    let a = l.a + b.a * (1.0 - l.a);
    if a <= 0.0 {
        return gpui::transparent_black();
    }
    let channel = |lc: f32, bc: f32| (lc * l.a + bc * b.a * (1.0 - l.a)) / a;
    gpui::Rgba {
        r: channel(l.r, b.r),
        g: channel(l.g, b.g),
        b: channel(l.b, b.b),
        a,
    }
    .into()
}

fn distance(a: Point<Pixels>, b: Point<Pixels>) -> f32 {
    let dx = f32::from(a.x - b.x);
    let dy = f32::from(a.y - b.y);
    (dx * dx + dy * dy).sqrt()
}

/// Register the repaint that keeps the light under the pointer.
///
/// `zone` is where the light is visible; `was` is where the pointer sat
/// when this frame painted. A move that starts or ends inside the zone
/// redraws the window — the "ends" half is what paints the frame with no
/// light once the pointer has left.
fn follow_pointer(zone: Bounds<Pixels>, was: Point<Pixels>, window: &mut Window) {
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _cx| {
        if phase != DispatchPhase::Capture {
            return;
        }
        if zone.contains(&event.position) || zone.contains(&was) {
            window.refresh();
        }
    });
}

// ------------------------------------------------------------------ highlight

/// See [`Reveal::reveal_highlight`].
pub struct RevealHighlight {
    child: Option<AnyElement>,
    radius: Option<f32>,
    style: Option<RevealStyle>,
}

impl RevealHighlight {
    pub fn new(child: impl IntoElement) -> Self {
        Self {
            child: Some(child.into_any_element()),
            radius: None,
            style: None,
        }
    }

    /// The corner radius the glow is clipped to. Match the element's own;
    /// defaults to the theme radius. Pass `0.` for a square surface.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = Some(radius);
        self
    }

    /// Override the theme-derived tuning.
    pub fn style(mut self, style: RevealStyle) -> Self {
        self.style = Some(style);
        self
    }
}

impl IntoElement for RevealHighlight {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for RevealHighlight {
    type RequestLayoutState = AnyElement;
    type PrepaintState = Hitbox;

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
        let mut child = self.child.take().expect("a reveal wrapper lays out once");
        let layout = child.request_layout(window, cx);
        (layout, child)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        // Normal, not blocking: this hitbox exists to be *asked* whether the
        // pointer is on the element and nothing above it — a scrim, a menu —
        // and must not change how anything else receives the mouse. Inserted
        // *after* the child's, so it sits above them: a child that occludes
        // (a modal card) would otherwise hide it from the hit test.
        child.prepaint(window, cx);
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let theme = cx.theme();
        let style = self.style.unwrap_or_else(|| RevealStyle::highlight(theme));
        let radius = self.radius.unwrap_or_else(|| theme.radius());
        let dark = theme.sunken();
        let pointer = window.mouse_position();

        // Under the child, so the element's own wash and text paint over it
        // untouched. Omarchy fills are alpha washes, so the glow shows
        // through them rather than being hidden.
        if hitbox.is_hovered(window) {
            let texture = cx.default_global::<GlowTextures>().get(style);
            let reach = px(style.reach);
            let image_bounds = Bounds {
                origin: pointer - point(reach, reach),
                size: size(reach * 2.0, reach * 2.0),
            };
            // The image is clipped to `bounds` with the element's radii: the
            // one place gpui offers a rounded clip.
            let _ = window.paint_image(
                bounds,
                image_bounds,
                Corners::all(px(radius)),
                texture,
                0,
                false,
            );
            paint_glow_grain(bounds, pointer, style, dark, window, cx);
        }

        child.paint(window, cx);
        follow_pointer(bounds, pointer, window);
    }
}

/// How many rings the glow's grain is stepped in.
const GRAIN_RINGS: u32 = 5;

/// Grain over the glow, denser toward the pointer.
///
/// The tile is one fixed texture, so it cannot fade on its own; the fade is
/// stepped instead — the same tile painted into concentric circles, the
/// widest at the reach and each one after a fifth smaller, so the specks
/// under the pointer carry five layers and those at the edge one. The
/// circles are `paint_image`'s rounded clip, a square with radii of half
/// its side, inside a mask of the element's own bounds.
fn paint_glow_grain(
    bounds: Bounds<Pixels>,
    pointer: Point<Pixels>,
    style: RevealStyle,
    dark: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    if style.grain <= 0.0 {
        return;
    }
    let grain = GrainStyle {
        light: style.color,
        dark,
        strength: style.grain,
    };
    let layer = style.grain / GRAIN_RINGS as f32;
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for ring in 0..GRAIN_RINGS {
            let radius = px(style.reach * (1.0 - ring as f32 / GRAIN_RINGS as f32));
            let circle = Bounds {
                origin: pointer - point(radius, radius),
                size: size(radius * 2.0, radius * 2.0),
            };
            paint_grain(circle, Corners::all(radius), grain, layer, window, cx);
        }
    });
}

/// The glow textures, one per colour and strength, built on first use.
///
/// A texture is 96 device pixels square and is stretched to the reach, so
/// there is no per-reach copy; the GPU's bilinear filter smooths the
/// stretch, and a glow with no hard edge has nothing to lose to it.
#[derive(Default)]
struct GlowTextures(HashMap<[u8; 4], Arc<RenderImage>>);

impl Global for GlowTextures {}

const GLOW_SIZE: u32 = 96;

impl GlowTextures {
    fn get(&mut self, style: RevealStyle) -> Arc<RenderImage> {
        let key = glow_key(style);
        self.0
            .entry(key)
            .or_insert_with(|| {
                let pixels = glow_pixels(GLOW_SIZE, key);
                let buffer = image::RgbaImage::from_raw(GLOW_SIZE, GLOW_SIZE, pixels)
                    .expect("glow buffer is exactly width * height * 4 bytes");
                Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
            })
            .clone()
    }
}

/// The texture's identity: colour and peak alpha, quantised to a byte each.
/// Returned in gpui's `RenderImage` byte order, which is BGRA.
fn glow_key(style: RevealStyle) -> [u8; 4] {
    let rgba: gpui::Rgba = style.color.into();
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [
        byte(rgba.b),
        byte(rgba.g),
        byte(rgba.r),
        byte(style.strength),
    ]
}

/// A radial glow: `bgra[3]` at the centre, transparent at the edge, with the
/// same easing as [`falloff`] so the border and the highlight fade alike.
/// Straight (not premultiplied) alpha, which is what gpui's sprite shader
/// expects from image data.
fn glow_pixels(size: u32, bgra: [u8; 4]) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    let half = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            let dx = (x as f32 + 0.5 - half) / half;
            let dy = (y as f32 + 0.5 - half) / half;
            let d = (dx * dx + dy * dy).sqrt();
            let alpha = falloff(d, 1.0, 1.0) * bgra[3] as f32;
            pixels.extend_from_slice(&[bgra[0], bgra[1], bgra[2], alpha.round() as u8]);
        }
    }
    pixels
}

// --------------------------------------------------------------------- border

/// See [`Reveal::reveal_border`].
pub struct RevealBorder {
    child: Option<AnyElement>,
    radius: Option<f32>,
    width: Option<f32>,
    style: Option<RevealStyle>,
    /// See [`Reveal::reveal_frame`]: the reach is the element's own size.
    frame: bool,
    /// See [`RevealBorder::base`].
    base: Option<Hsla>,
}

impl RevealBorder {
    pub fn new(child: impl IntoElement) -> Self {
        Self {
            child: Some(child.into_any_element()),
            radius: None,
            width: None,
            style: None,
            frame: false,
            base: None,
        }
    }

    /// Make this outline *the* element's border: always drawn, in `base`
    /// where the light does not reach and blending up to the light where
    /// it does. The element should then draw no border colour of its own,
    /// or the gradient rides on a solid line and reads as one.
    pub fn base(mut self, base: Hsla) -> Self {
        self.base = Some(base);
        self
    }

    /// Make this outline the element's border in `color`: lit to
    /// [`FRAME_LIGHT`] of its alpha where the light reaches, dimmed to
    /// [`FRAME_BASE`] where it does not. The theme's own border colour is
    /// what both are taken from, so the border under the pointer is a
    /// softened version of what the theme draws, fading away from it.
    pub fn edge(self, color: Hsla) -> Self {
        let style = RevealStyle {
            color,
            strength: FRAME_LIGHT,
            ..self.style.unwrap_or(RevealStyle {
                color,
                reach: 0.0,
                strength: FRAME_LIGHT,
                grain: 0.0,
            })
        };
        self.base(color.opacity(FRAME_BASE)).style(style)
    }

    /// Light the outline across the element's whole size rather than a
    /// fixed reach — see [`Reveal::reveal_frame`]. Defaults the style to
    /// [`RevealStyle::frame`].
    pub fn frame(mut self) -> Self {
        self.frame = true;
        self
    }

    /// The reach this element lights over: its own larger side for a
    /// frame, the style's otherwise.
    fn reach(&self, bounds: Bounds<Pixels>, style: RevealStyle) -> f32 {
        if self.frame {
            // Past half the card the light is gone: from the middle, the
            // near edge is bright and the corners have faded to the base.
            f32::from(bounds.size.width).max(f32::from(bounds.size.height)) * FRAME_REACH
        } else {
            style.reach
        }
    }

    fn resolved_style(&self, theme: &Theme) -> RevealStyle {
        self.style.unwrap_or_else(|| {
            if self.frame {
                RevealStyle::frame(theme)
            } else {
                RevealStyle::border(theme)
            }
        })
    }

    /// The corner radius of the outline. Match the element's own; defaults
    /// to the theme radius.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = Some(radius);
        self
    }

    /// The outline's width. Defaults to the theme's control border, at
    /// least a pixel.
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// Override the theme-derived tuning.
    pub fn style(mut self, style: RevealStyle) -> Self {
        self.style = Some(style);
        self
    }
}

impl IntoElement for RevealBorder {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// How much of the border's strength a *nearby* pointer earns, against a
/// pointer on the element itself.
const NEAR_STRENGTH: f32 = 0.5;

/// A frame's reach, as a fraction of the container's larger side.
const FRAME_REACH: f32 = 0.6;

/// How much of an [`RevealBorder::edge`] colour's alpha remains where the
/// light does not reach.
pub const FRAME_BASE: f32 = 0.3;

/// How much of an [`RevealBorder::edge`] colour the light adds at the
/// pointer, over the base.
pub const FRAME_LIGHT: f32 = 0.5;

/// What the border remembers between prepaint and paint.
pub struct BorderPrepaint {
    /// Covers the element *and* its reach, so `is_hovered` answers "is the
    /// pointer near, with nothing blocking in front" in one query.
    zone: Hitbox,
    /// The reach the zone was built with, so paint lights the same span.
    reach: f32,
}

impl Element for RevealBorder {
    type RequestLayoutState = AnyElement;
    type PrepaintState = BorderPrepaint;

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
        let mut child = self.child.take().expect("a reveal wrapper lays out once");
        let layout = child.request_layout(window, cx);
        (layout, child)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let reach = self.reach(bounds, self.resolved_style(cx.theme()));
        // After the child, above its hitboxes — see `RevealHighlight`.
        child.prepaint(window, cx);
        let zone = window.insert_hitbox(bounds.dilate(px(reach)), HitboxBehavior::Normal);
        BorderPrepaint { zone, reach }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let theme = cx.theme();
        let style = RevealStyle {
            reach: prepaint.reach,
            ..self.resolved_style(theme)
        };
        let radius = self.radius.unwrap_or_else(|| theme.radius());
        let width = self.width.unwrap_or_else(|| theme.border_width().max(1.0));
        let pointer = window.mouse_position();

        child.paint(window, cx);

        // Over the child: the lit outline sits on top of the element's own
        // border and brightens it where the pointer is. A neighbour lights
        // at half strength — enough to say the surface is one, not enough
        // to compete with the button the pointer is actually on.
        let hovered = prepaint.zone.is_hovered(window);
        if hovered || self.base.is_some() {
            let strength = if !hovered {
                // Not lit at all — a keyboard user, or a pointer elsewhere
                // — so only the base is drawn.
                0.0
            } else if bounds.contains(&pointer) {
                style.strength
            } else {
                style.strength * NEAR_STRENGTH
            };
            let style = RevealStyle { strength, ..style };
            // A frame is normalised to the pointer: the nearest point of
            // the outline is always fully lit and the farthest corner is
            // always at the base, wherever the pointer sits over the card.
            // A fixed reach would leave the whole outline dim from the
            // middle of a large card, where every edge is far.
            let (near, style) = if self.frame {
                let (near, far) = outline_distances(bounds, pointer);
                (
                    near,
                    RevealStyle {
                        reach: (far - near).max(1.0),
                        ..style
                    },
                )
            } else {
                (0.0, style)
            };
            for piece in outline_pieces(bounds, radius, width, pointer, style, near, self.base) {
                window.paint_quad(piece.into_quad());
            }
        }

        follow_pointer(bounds.dilate(px(style.reach)), pointer, window);
    }
}

/// One quad of a lit outline. Pure data, so the geometry can be tested
/// without a window.
#[derive(Debug, Clone, PartialEq)]
enum OutlinePiece {
    /// A straight run of one edge, brightening (or fading) along its length.
    Edge {
        bounds: Bounds<Pixels>,
        /// gpui's gradient angle: 90 runs left→right, 180 top→bottom.
        angle: f32,
        from: Hsla,
        to: Hsla,
    },
    /// A rounded corner, at one brightness.
    Corner {
        bounds: Bounds<Pixels>,
        radii: Corners<Pixels>,
        widths: Edges<Pixels>,
        color: Hsla,
    },
}

impl OutlinePiece {
    fn into_quad(self) -> gpui::PaintQuad {
        match self {
            Self::Edge {
                bounds,
                angle,
                from,
                to,
            } => fill(
                bounds,
                linear_gradient(
                    angle,
                    linear_color_stop(from, 0.),
                    linear_color_stop(to, 1.),
                ),
            ),
            Self::Corner {
                bounds,
                radii,
                widths,
                color,
            } => quad(
                bounds,
                radii,
                Background::from(transparent_black()),
                widths,
                color,
                BorderStyle::Solid,
            ),
        }
    }
}

/// How far the pointer is from the nearest point of `bounds`' outline, and
/// from its farthest corner. Zero for the first when the pointer is on the
/// outline; from inside, the distance to the closest edge.
fn outline_distances(bounds: Bounds<Pixels>, pointer: Point<Pixels>) -> (f32, f32) {
    let (left, top, right, bottom) = (
        f32::from(bounds.left()),
        f32::from(bounds.top()),
        f32::from(bounds.right()),
        f32::from(bounds.bottom()),
    );
    let (x, y) = (f32::from(pointer.x), f32::from(pointer.y));
    let inside = x >= left && x <= right && y >= top && y <= bottom;
    let near = if inside {
        (x - left).min(right - x).min(y - top).min(bottom - y)
    } else {
        let dx = (left - x).max(0.0).max(x - right);
        let dy = (top - y).max(0.0).max(y - bottom);
        (dx * dx + dy * dy).sqrt()
    };
    let far = [(left, top), (right, top), (right, bottom), (left, bottom)]
        .into_iter()
        .map(|(cx, cy)| distance(pointer, point(px(cx), px(cy))))
        .fold(0.0, f32::max);
    (near, far)
}

/// The outline of `bounds`, `width` thick and drawn inside it like a CSS
/// border, lit from `pointer` — over nothing, or over a `base` colour that
/// the light blends onto, so the outline is a complete border on its own.
/// `near` is subtracted from every distance before the falloff: a frame
/// passes the distance to its nearest edge, so that edge starts at full
/// strength.
///
/// Each straight edge is split at the pointer's projection onto it, so a
/// two-stop gradient per half gives a peak under the pointer and a fade
/// away from it on both sides. Each rounded corner is one solid arc at the
/// brightness of its midpoint — a corner is a few pixels, and a gradient
/// across it would not be seen.
fn outline_pieces(
    bounds: Bounds<Pixels>,
    radius: f32,
    width: f32,
    pointer: Point<Pixels>,
    style: RevealStyle,
    near: f32,
    base: Option<Hsla>,
) -> Vec<OutlinePiece> {
    let (left, top, right, bottom) = (
        f32::from(bounds.left()),
        f32::from(bounds.top()),
        f32::from(bounds.right()),
        f32::from(bounds.bottom()),
    );
    let w = width.max(0.0);
    let (bw, bh) = (right - left, bottom - top);
    // A radius larger than half the box is not drawable; a radius smaller
    // than the border would draw the arc inside the run.
    let r = radius
        .clamp(0.0, (bw.min(bh) / 2.0).max(0.0))
        .max(if radius > 0.0 { w } else { 0.0 });
    let half = w / 2.0;

    // The brightness of the outline at a point on its centreline.
    let lit = |x: f32, y: f32| {
        let d = distance(pointer, point(px(x), px(y)));
        let light = style
            .color
            .opacity(falloff(d - near, style.reach, style.strength));
        match base {
            Some(base) => over(light, base),
            None => light,
        }
    };
    let (px_, py_) = (f32::from(pointer.x), f32::from(pointer.y));

    let mut pieces = Vec::with_capacity(12);

    // The horizontal runs, split at the pointer's x.
    let mut horizontal = |y_top: f32, y_line: f32| {
        let (x0, x1) = (left + r, right - r);
        let split = px_.clamp(x0, x1);
        for (a, b) in [(x0, split), (split, x1)] {
            if b - a <= 0.0 {
                continue;
            }
            pieces.push(OutlinePiece::Edge {
                bounds: Bounds {
                    origin: point(px(a), px(y_top)),
                    size: size(px(b - a), px(w)),
                },
                angle: 90.0,
                from: lit(a, y_line),
                to: lit(b, y_line),
            });
        }
    };
    horizontal(top, top + half);
    horizontal(bottom - w, bottom - half);

    // The vertical runs, split at the pointer's y.
    let mut vertical = |x_left: f32, x_line: f32| {
        let (y0, y1) = (top + r, bottom - r);
        let split = py_.clamp(y0, y1);
        for (a, b) in [(y0, split), (split, y1)] {
            if b - a <= 0.0 {
                continue;
            }
            pieces.push(OutlinePiece::Edge {
                bounds: Bounds {
                    origin: point(px(x_left), px(a)),
                    size: size(px(w), px(b - a)),
                },
                angle: 180.0,
                from: lit(x_line, a),
                to: lit(x_line, b),
            });
        }
    };
    vertical(left, left + half);
    vertical(right - w, right - half);

    if r > 0.0 {
        // The arc's midpoint sits r·(1 − 1/√2) in from each of its two
        // edges, on the centreline of the stroke.
        let inset = r - (r - half) * std::f32::consts::FRAC_1_SQRT_2;
        let corner =
            |x: f32, y: f32, mid: (f32, f32), radii: Corners<Pixels>, widths: Edges<Pixels>| {
                OutlinePiece::Corner {
                    bounds: Bounds {
                        origin: point(px(x), px(y)),
                        size: size(px(r), px(r)),
                    },
                    radii,
                    widths,
                    color: lit(mid.0, mid.1),
                }
            };
        let zero = px(0.);
        let (rp, wp) = (px(r), px(w));
        pieces.push(corner(
            left,
            top,
            (left + inset, top + inset),
            Corners {
                top_left: rp,
                top_right: zero,
                bottom_right: zero,
                bottom_left: zero,
            },
            Edges {
                top: wp,
                right: zero,
                bottom: zero,
                left: wp,
            },
        ));
        pieces.push(corner(
            right - r,
            top,
            (right - inset, top + inset),
            Corners {
                top_left: zero,
                top_right: rp,
                bottom_right: zero,
                bottom_left: zero,
            },
            Edges {
                top: wp,
                right: wp,
                bottom: zero,
                left: zero,
            },
        ));
        pieces.push(corner(
            right - r,
            bottom - r,
            (right - inset, bottom - inset),
            Corners {
                top_left: zero,
                top_right: zero,
                bottom_right: rp,
                bottom_left: zero,
            },
            Edges {
                top: zero,
                right: wp,
                bottom: wp,
                left: zero,
            },
        ));
        pieces.push(corner(
            left,
            bottom - r,
            (left + inset, bottom - inset),
            Corners {
                top_left: zero,
                top_right: zero,
                bottom_right: zero,
                bottom_left: rp,
            },
            Edges {
                top: zero,
                right: zero,
                bottom: wp,
                left: wp,
            },
        ));
    }

    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falloff_peaks_at_the_pointer_and_ends_at_the_reach() {
        assert_eq!(falloff(0.0, 100.0, 0.5), 0.5);
        assert_eq!(falloff(100.0, 100.0, 0.5), 0.0);
        assert_eq!(falloff(250.0, 100.0, 0.5), 0.0);
        assert_eq!(falloff(10.0, 0.0, 0.5), 0.0);

        // Monotone in between: a light that brightens away from the pointer
        // would read as a second light.
        let mut last = f32::INFINITY;
        for step in 0..=20 {
            let value = falloff(step as f32 * 5.0, 100.0, 1.0);
            assert!(value <= last, "step {step}: {value} > {last}");
            last = value;
        }
    }

    #[test]
    fn the_glow_texture_is_bgra_with_straight_alpha() {
        let size = 8;
        let pixels = glow_pixels(size, [10, 20, 30, 200]);
        assert_eq!(pixels.len(), (size * size * 4) as usize);

        // Every pixel carries the colour untouched — straight alpha, not
        // premultiplied — and only the alpha varies.
        for pixel in pixels.chunks_exact(4) {
            assert_eq!(&pixel[..3], &[10, 20, 30]);
        }
        let alpha = |x: u32, y: u32| pixels[((y * size + x) * 4 + 3) as usize];
        // Brightest at the centre, dark at the corner, and lit somewhere in
        // between.
        assert!(
            alpha(4, 4) > alpha(2, 2),
            "{} vs {}",
            alpha(4, 4),
            alpha(2, 2)
        );
        assert!(alpha(2, 2) > alpha(0, 0));
        assert_eq!(alpha(0, 0), 0);
    }

    #[test]
    fn the_glow_key_is_bgra_and_quantised() {
        let style = RevealStyle {
            color: gpui::Rgba {
                r: 1.0,
                g: 0.5,
                b: 0.0,
                a: 1.0,
            }
            .into(),
            reach: 10.0,
            strength: 0.5,
            grain: 0.0,
        };
        let [b, g, r, a] = glow_key(style);
        assert_eq!((r, b), (255, 0));
        assert!((126..=129).contains(&g), "{g}");
        assert!((127..=128).contains(&a), "{a}");
    }

    fn style() -> RevealStyle {
        RevealStyle {
            color: gpui::white(),
            reach: 100.0,
            strength: 1.0,
            grain: 0.0,
        }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        }
    }

    #[test]
    fn a_square_outline_is_eight_runs_split_under_the_pointer() {
        let pieces = outline_pieces(
            rect(0., 0., 100., 40.),
            0.0,
            1.0,
            point(px(30.), px(10.)),
            style(),
            0.0,
            None,
        );
        assert_eq!(pieces.len(), 8);
        assert!(
            pieces
                .iter()
                .all(|p| matches!(p, OutlinePiece::Edge { .. }))
        );

        // The top edge's two runs meet at x = 30, and the run's peak — its
        // shared stop — is the brightest colour on that edge.
        let top: Vec<_> = pieces
            .iter()
            .filter_map(|p| match p {
                OutlinePiece::Edge {
                    bounds,
                    angle,
                    from,
                    to,
                } if *angle == 90.0 && bounds.top() == px(0.) => {
                    Some((bounds.left(), bounds.right(), from.a, to.a))
                }
                _ => None,
            })
            .collect();
        assert_eq!(top.len(), 2);
        assert_eq!((top[0].0, top[0].1), (px(0.), px(30.)));
        assert_eq!((top[1].0, top[1].1), (px(30.), px(100.)));
        assert!(top[0].3 > top[0].2, "left run brightens toward the pointer");
        assert!(top[1].2 > top[1].3, "right run fades away from it");
        assert_eq!(top[0].3, top[1].2, "the two runs share their peak");
    }

    #[test]
    fn a_rounded_outline_adds_a_corner_per_arc_and_shortens_the_runs() {
        let pieces = outline_pieces(
            rect(0., 0., 100., 40.),
            6.0,
            1.0,
            point(px(-500.), px(-500.)),
            style(),
            0.0,
            None,
        );
        let corners = pieces
            .iter()
            .filter(|p| matches!(p, OutlinePiece::Corner { .. }))
            .count();
        assert_eq!(corners, 4);
        // Pointer clamped to the run's start: one run per edge is empty and
        // skipped, so four runs and four corners.
        assert_eq!(pieces.len(), 8);
        for piece in &pieces {
            if let OutlinePiece::Edge { bounds, angle, .. } = piece {
                if *angle == 90.0 {
                    assert_eq!((bounds.left(), bounds.right()), (px(6.), px(94.)));
                } else {
                    assert_eq!((bounds.top(), bounds.bottom()), (px(6.), px(34.)));
                }
            }
        }
    }

    #[test]
    fn a_base_is_the_border_where_the_light_does_not_reach() {
        let base = gpui::red();
        let pieces = outline_pieces(
            rect(0., 0., 100., 40.),
            4.0,
            1.0,
            point(px(900.), px(900.)),
            style(),
            0.0,
            Some(base),
        );
        for piece in &pieces {
            match piece {
                OutlinePiece::Edge { from, to, .. } => {
                    assert_eq!(*from, base);
                    assert_eq!(*to, base);
                }
                OutlinePiece::Corner { color, .. } => assert_eq!(*color, base),
            }
        }
        // And under the pointer the light lifts it off the base.
        let lit = outline_pieces(
            rect(0., 0., 100., 40.),
            0.0,
            1.0,
            point(px(0.), px(0.)),
            style(),
            0.0,
            Some(base),
        );
        let brightest = lit
            .iter()
            .filter_map(|p| match p {
                OutlinePiece::Edge { from, to, .. } => Some(from.l.max(to.l)),
                _ => None,
            })
            .fold(0.0, f32::max);
        assert!(brightest > base.l, "{brightest} vs {}", base.l);
    }

    #[test]
    fn outline_distances_measure_the_nearest_edge_and_farthest_corner() {
        let bounds = rect(0., 0., 100., 40.);
        // Inside: 10 from the top edge, farthest corner is bottom-right.
        let (near, far) = outline_distances(bounds, point(px(30.), px(10.)));
        assert_eq!(near, 10.0);
        assert!((far - (70.0f32 * 70.0 + 30.0 * 30.0).sqrt()).abs() < 1e-3);
        // On the outline.
        assert_eq!(outline_distances(bounds, point(px(0.), px(20.))).0, 0.0);
        // Outside, straight below the bottom edge.
        assert_eq!(outline_distances(bounds, point(px(50.), px(60.))).0, 20.0);
    }

    #[test]
    fn light_over_a_translucent_base_raises_its_alpha() {
        let base = gpui::white().opacity(0.3);
        assert_eq!(over(gpui::white().opacity(0.0), base).a, 0.3);
        let half = over(gpui::white().opacity(0.5), base).a;
        assert!((half - 0.65).abs() < 1e-5, "{half}");
        assert_eq!(over(gpui::white(), base).a, 1.0);
        // gpui's own blend is what this replaces: it would keep 0.3.
        assert_eq!(base.blend(gpui::white().opacity(0.5)).a, 0.3);
    }

    #[test]
    fn a_far_pointer_leaves_the_outline_dark() {
        let pieces = outline_pieces(
            rect(0., 0., 100., 40.),
            4.0,
            1.0,
            point(px(900.), px(900.)),
            style(),
            0.0,
            None,
        );
        for piece in pieces {
            match piece {
                OutlinePiece::Edge { from, to, .. } => {
                    assert_eq!(from.a, 0.0);
                    assert_eq!(to.a, 0.0);
                }
                OutlinePiece::Corner { color, .. } => assert_eq!(color.a, 0.0),
            }
        }
    }
}

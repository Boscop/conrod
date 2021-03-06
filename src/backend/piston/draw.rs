//! A piston backend for rendering conrod primitives.

extern crate graphics as piston_graphics;

use Rect;
use image;
use render;
use text;

#[doc(inline)]
pub use self::piston_graphics::{Context, DrawState, Graphics, ImageSize, Transformed};


/// Render the given sequence of conrod primitive widgets.
///
/// Params:
///
/// - `primitives` - The sequence of primitives to be rendered to the screen.
/// - `context` - The piston2d-graphics drawing context.
/// - `graphics` - The piston `Graphics` backend.
/// - `text_texture_cache` - Some texture type `T` upon which we can cache text glyphs.
/// - `glyph_cache` - The RustType `Cache` used to cache glyphs in our `text_texture_cache`.
/// - `image_map` - Mappings from image widget indices to their associated image data.
/// - `cache_queue_glyphs` - A function for caching glyphs within the given texture cache.
/// - `texture_from_image` - A function that borrows a drawable texture `T` from an `Img`. In many
///   cases, `Img` may be the same type as `T`, however we provide this to allow for flexibility.
pub fn primitives<'a, P, G, T, Img, C, F>(
    mut primitives: P,
    context: Context,
    graphics: &'a mut G,
    text_texture_cache: &'a mut T,
    glyph_cache: &'a mut text::GlyphCache,
    image_map: &'a image::Map<Img>,
    mut cache_queued_glyphs: C,
    mut texture_from_image: F,
)
    where P: render::PrimitiveWalker,
          G: Graphics<Texture=T>,
          T: ImageSize,
          C: FnMut(&mut G, &mut T, text::rt::Rect<u32>, &[u8]),
          F: FnMut(&Img) -> &T,
{

    // A re-usable buffer of rectangles describing the glyph's screen and texture positions.
    let mut glyph_rectangles = Vec::new();

    while let Some(prim) = render::PrimitiveWalker::next_primitive(&mut primitives) {
        primitive(prim,
                  context,
                  graphics,
                  text_texture_cache,
                  glyph_cache,
                  image_map,
                  &mut glyph_rectangles,
                  &mut cache_queued_glyphs,
                  &mut texture_from_image);
    }
}


/// Render a single `Primitive`.
///
/// Params:
///
/// - `primitive` - The `Primitive` that is to be rendered to the screen.
/// - `context` - The piston2d-graphics drawing context.
/// - `graphics` - The piston `Graphics` backend.
/// - `text_texture_cache` - Some texture type `T` upon which we can cache text glyphs.
/// - `glyph_cache` - The RustType `Cache` used to cache glyphs in our `text_texture_cache`.
/// - `image_map` - Mappings from image widget indices to their associated image data.
/// - `glyph_rectangles` - A re-usable buffer for collecting positioning rectangles for glyphs.
/// - `cache_queue_glyphs` - A function for caching glyphs within the given texture cache.
/// - `texture_from_image` - A function that borrows a drawable texture `T` from an `Img`. In many
///   cases, `Img` may be the same type as `T`, however we provide this to allow for flexibility.
pub fn primitive<'a, Img, G, T, C, F>(
    primitive: render::Primitive,
    context: Context,
    graphics: &'a mut G,
    text_texture_cache: &'a mut T,
    glyph_cache: &'a mut text::GlyphCache,
    image_map: &'a image::Map<Img>,
    glyph_rectangles: &mut Vec<([f64; 4], [i32; 4])>,
    mut cache_queued_glyphs: C,
    mut texture_from_image: F,
)
    where G: Graphics<Texture=T>,
          T: ImageSize,
          C: FnMut(&mut G, &mut T, text::rt::Rect<u32>, &[u8]),
          F: FnMut(&Img) -> &T,
{
    let render::Primitive { id, kind, scizzor, rect } = primitive;
    let view_size = context.get_view_size();
    // Translate the `context` to suit conrod's orientation (middle (0, 0), y pointing upwards).
    let context = context.trans(view_size[0] / 2.0, view_size[1] / 2.0).scale(1.0, -1.0);
    let context = crop_context(context, scizzor);

    match kind {

        render::PrimitiveKind::Rectangle { color } => {
            let (l, b, w, h) = rect.l_b_w_h();
            let lbwh = [l, b, w, h];
            let rectangle = piston_graphics::Rectangle::new(color.to_fsa());
            rectangle.draw(lbwh, &context.draw_state, context.transform, graphics);
        },

        render::PrimitiveKind::Polygon { color, points } => {
            let color = color.to_fsa();
            let polygon = piston_graphics::Polygon::new(color);
            polygon.draw(points, &context.draw_state, context.transform, graphics);
        },

        render::PrimitiveKind::Lines { color, cap, thickness, points } => {
            use widget::primitive::line::Cap;
            let color = color.to_fsa();

            let mut points = points.iter();
            if let Some(first) = points.next() {
                let line = match cap {
                    Cap::Flat => piston_graphics::Line::new(color, thickness / 2.0),
                    Cap::Round => piston_graphics::Line::new_round(color, thickness / 2.0),
                };
                let mut start = first;
                for end in points {
                    let coords = [start[0], start[1], end[0], end[1]];
                    line.draw(coords, &context.draw_state, context.transform, graphics);
                    start = end;
                }
            }
        },

        render::PrimitiveKind::Text { color, text, font_id } => {

            // Retrieve the "dots per inch" factor by dividing the window width by the view.
            //
            // TODO: Perhaps this should be a method on the `Context` type?
            let dpi_factor = context.viewport
                .map(|v| v.window_size[0] as f32 / view_size[0] as f32)
                .unwrap_or(1.0);
            let positioned_glyphs = text.positioned_glyphs(dpi_factor);
            // Re-orient the context to top-left origin with *y* facing downwards, as the
            // `positioned_glyphs` yield pixel positioning.
            let context = context.scale(1.0, -1.0).trans(-view_size[0] / 2.0, -view_size[1] / 2.0);

            // Queue the glyphs to be cached.
            for glyph in positioned_glyphs.iter() {
                glyph_cache.queue_glyph(font_id.index(), glyph.clone());
            }

            // Cache the glyphs within the GPU cache.
            glyph_cache.cache_queued(|rect, data| {
                cache_queued_glyphs(graphics, text_texture_cache, rect, data)
            }).unwrap();

            let cache_id = font_id.index();
            let (tex_w, tex_h) = text_texture_cache.get_size();
            let color = color.to_fsa();

            let rectangles = positioned_glyphs.into_iter()
                .filter_map(|g| glyph_cache.rect_for(cache_id, g).ok().unwrap_or(None))
                .map(|(uv_rect, screen_rect)| {
                    let rectangle = {
                        let div_dpi_factor = |s| (s as f32 / dpi_factor as f32) as f64;
                        let left = div_dpi_factor(screen_rect.min.x);
                        let top = div_dpi_factor(screen_rect.min.y);
                        let right = div_dpi_factor(screen_rect.max.x);
                        let bottom = div_dpi_factor(screen_rect.max.y);
                        let w = right - left;
                        let h = bottom - top;
                        [left, top, w, h]
                    };
                    let source_rectangle = {
                        let x = (uv_rect.min.x * tex_w as f32).round() as i32;
                        let y = (uv_rect.min.y * tex_h as f32).round() as i32;
                        let w = ((uv_rect.max.x - uv_rect.min.x) * tex_w as f32).round() as i32;
                        let h = ((uv_rect.max.y - uv_rect.min.y) * tex_h as f32).round() as i32;
                        [x, y, w, h]
                    };
                    (rectangle, source_rectangle)
                });
            glyph_rectangles.clear();
            glyph_rectangles.extend(rectangles);
            piston_graphics::image::draw_many(&glyph_rectangles,
                                              color,
                                              text_texture_cache,
                                              &context.draw_state,
                                              context.transform,
                                              graphics);
        },

        render::PrimitiveKind::Image { color, source_rect } => {
            if let Some(img) = image_map.get(&id) {
                let mut image = piston_graphics::image::Image::new();
                image.color = color.map(|c| c.to_fsa());
                if let Some(source_rect) = source_rect {
                    let (x, y, w, h) = source_rect.x_y_w_h();
                    image.source_rectangle = Some([x as i32, y as i32, w as i32, h as i32]);
                }
                let (left, top, w, h) = rect.l_t_w_h();
                image.rectangle = Some([0.0, 0.0, w, h]);
                let context = context.trans(left, top).scale(1.0, -1.0);
                let transform = context.transform;
                let draw_state = &context.draw_state;
                let tex = texture_from_image(img);
                image.draw(tex, draw_state, transform, graphics);
            }
        },

        render::PrimitiveKind::Other(_widget) => {
            // TODO: Perhaps add a function to the `primitives` params to allow a user to
            // handle these.
        },

    }
 

}



/// Crop the given **Context** to the given **Rect**.
///
/// This is non-trivial as we must consider the view_size, viewport, the difference in
/// co-ordinate systems and the conversion from `f64` dimensions to `u16`.
fn crop_context(context: Context, rect: Rect) -> Context {
    use utils::map_range;
    let Context { draw_state, .. } = context;

    let (x, y, w, h) = rect.x_y_w_h();

    // Our view_dim is our virtual window size which is consistent no matter the display.
    let view_dim = context.get_view_size();

    // Our draw_dim is the actual window size in pixels. Our target crop area must be
    // represented in this size.
    let draw_dim = match context.viewport {
        Some(viewport) => [viewport.draw_size[0] as f64, viewport.draw_size[1] as f64],
        None => view_dim,
    };

    // Calculate the distance to the edges of the window from the center.
    let left = -view_dim[0] / 2.0;
    let right = view_dim[0] / 2.0;
    let bottom = -view_dim[1] / 2.0;
    let top = view_dim[1] / 2.0;

    // We start with the x and y in the center of our crop area, however we need it to be
    // at the top left of the crop area.
    let left_x = x - w as f64 / 2.0;
    let top_y = y - h as f64 / 2.0;

    // Map the position at the top left of the crop area in view_dim to our draw_dim.
    let x = map_range(left_x, left, right, 0, draw_dim[0] as i32);
    let y = map_range(top_y, bottom, top, 0, draw_dim[1] as i32);

    // Convert the w and h from our view_dim to the draw_dim.
    let w_scale = draw_dim[0] / view_dim[0];
    let h_scale = draw_dim[1] / view_dim[1];
    let w = w * w_scale;
    let h = h * h_scale;

    // If we ended up with negative coords for the crop area, we'll use 0 instead as we
    // can't represent the negative coords with `u32` (the target DrawState dimension type).
    // We'll hold onto the lost negative values (x_neg and y_neg) so that we can compensate
    // with the width and height.
    let x_neg = if x < 0 { x } else { 0 };
    let y_neg = if y < 0 { y } else { 0 };
    let mut x = ::std::cmp::max(0, x) as u32;
    let mut y = ::std::cmp::max(0, y) as u32;
    let mut w = ::std::cmp::max(0, (w as i32 + x_neg)) as u32;
    let mut h = ::std::cmp::max(0, (h as i32 + y_neg)) as u32;

    // If there was already some scissor set, we must check for the intersection.
    if let Some(rect) = draw_state.scissor {
        let (r_x, r_y, r_w, r_h) = (rect[0], rect[1], rect[2], rect[3]);
        if x + w < r_x || r_x + r_w < x || y + h < r_y || r_y + r_h < y {
            // If there is no intersection, we have no scissor.
            w = 0;
            h = 0;
        } else {
            // If there is some intersection, calculate the overlapping rect.
            let (a_l, a_r, a_b, a_t) = (x, x+w, y, y+h);
            let (b_l, b_r, b_b, b_t) = (r_x, r_x+r_w, r_y, r_y+r_h);
            let l = if a_l > b_l { a_l } else { b_l };
            let r = if a_r < b_r { a_r } else { b_r };
            let b = if a_b > b_b { a_b } else { b_b };
            let t = if a_t < b_t { a_t } else { b_t };
            x = l;
            y = b;
            w = r - l;
            h = t - b;
        }
    }

    Context { draw_state: draw_state.scissor([x, y, w, h]), ..context }
}

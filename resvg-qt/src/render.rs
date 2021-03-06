// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use log::warn;

pub(crate) mod prelude {
    pub(crate) use usvg::*;
    pub(crate) use crate::layers::Layers;
    pub(crate) use crate::qt;
    pub(crate) use super::*;
}

use prelude::*;


/// Indicates the current rendering state.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum RenderState {
    /// A default value. Doesn't indicate anything.
    Ok,
    /// Indicates that the current rendering task should stop after reaching the specified node.
    RenderUntil(usvg::Node),
    /// Indicates that `usvg::FilterInput::BackgroundImage` rendering task was finished.
    BackgroundFinished,
}


pub(crate) trait ConvTransform<T> {
    fn to_native(&self) -> T;
    fn from_native(_: &T) -> Self;
}

impl ConvTransform<qt::Transform> for usvg::Transform {
    fn to_native(&self) -> qt::Transform {
        qt::Transform::new(self.a, self.b, self.c, self.d, self.e, self.f)
    }

    fn from_native(ts: &qt::Transform) -> Self {
        let d = ts.data();
        Self::new(d.0, d.1, d.2, d.3, d.4, d.5)
    }
}


pub(crate) fn render_node_to_canvas(
    node: &usvg::Node,
    view_box: usvg::ViewBox,
    img_size: ScreenSize,
    state: &mut RenderState,
    painter: &mut qt::Painter,
) {
    let mut layers = Layers::new(img_size);

    apply_viewbox_transform(view_box, img_size, painter);

    let curr_ts = painter.get_transform();

    let mut ts = node.abs_transform();
    ts.append(&node.transform());

    painter.apply_transform(&ts.to_native());
    render_node(node, state, &mut layers, painter);
    painter.set_transform(&curr_ts);
}

pub(crate) fn create_root_image(
    size: ScreenSize,
    fit_to: usvg::FitTo,
    background: Option<usvg::Color>,
) -> Option<(qt::Image, ScreenSize)> {
    let img_size = fit_to.fit_to(size)?;

    let mut img = create_subimage(img_size)?;

    // Fill background.
    if let Some(c) = background {
        img.fill(c.red, c.green, c.blue, 255);
    }

    Some((img, img_size))
}

/// Applies viewbox transformation to the painter.
fn apply_viewbox_transform(
    view_box: usvg::ViewBox,
    img_size: ScreenSize,
    painter: &mut qt::Painter,
) {
    let ts = usvg::utils::view_box_to_transform(view_box.rect, view_box.aspect, img_size.to_size());
    painter.apply_transform(&ts.to_native());
}

pub(crate) fn render_node(
    node: &usvg::Node,
    state: &mut RenderState,
    layers: &mut Layers,
    p: &mut qt::Painter,
) -> Option<Rect> {
    match *node.borrow() {
        usvg::NodeKind::Svg(_) => {
            render_group(node, state, layers, p)
        }
        usvg::NodeKind::Path(ref path) => {
            crate::path::draw(&node.tree(), path, p)
        }
        usvg::NodeKind::Image(ref img) => {
            Some(crate::image::draw(img, p))
        }
        usvg::NodeKind::Group(ref g) => {
            render_group_impl(node, g, state, layers, p)
        }
        _ => None,
    }
}

pub(crate) fn render_group(
    parent: &usvg::Node,
    state: &mut RenderState,
    layers: &mut Layers,
    p: &mut qt::Painter,
) -> Option<Rect> {
    let curr_ts = p.get_transform();
    let mut g_bbox = Rect::new_bbox();

    for node in parent.children() {
        match state {
            RenderState::Ok => {}
            RenderState::RenderUntil(ref last) => {
                if node == *last {
                    // Stop rendering.
                    *state = RenderState::BackgroundFinished;
                    break;
                }
            }
            RenderState::BackgroundFinished => break,
        }

        p.apply_transform(&node.transform().to_native());

        let bbox = render_node(&node, state, layers, p);
        if let Some(bbox) = bbox {
            if let Some(bbox) = bbox.transform(&node.transform()) {
                g_bbox = g_bbox.expand(bbox);
            }
        }

        // Revert transform.
        p.set_transform(&curr_ts);
    }

    // Check that bbox was changed, otherwise we will have a rect with x/y set to f64::MAX.
    if g_bbox.fuzzy_ne(&Rect::new_bbox()) {
        Some(g_bbox)
    } else {
        None
    }
}

fn render_group_impl(
    node: &usvg::Node,
    g: &usvg::Group,
    state: &mut RenderState,
    layers: &mut Layers,
    p: &mut qt::Painter,
) -> Option<Rect> {
    let sub_img = layers.get()?;
    let mut sub_img = sub_img.borrow_mut();

    let curr_ts = p.get_transform();

    let bbox = {
        let mut sub_p = qt::Painter::new(&mut sub_img);
        sub_p.set_transform(&curr_ts);

        render_group(node, state, layers, &mut sub_p)
    };

    // During the background rendering for filters,
    // an opacity, a filter, a clip and a mask should be ignored for the inner group.
    // So we are simply rendering the `sub_img` without any postprocessing.
    //
    // SVG spec, 15.6 Accessing the background image
    // 'Any filter effects, masking and group opacity that might be set on A[i] do not apply
    // when rendering the children of A[i] into BUF[i].'
    if *state == RenderState::BackgroundFinished {
        let curr_ts = p.get_transform();
        p.set_transform(&qt::Transform::default());
        p.draw_image(0.0, 0.0, &sub_img);
        p.set_transform(&curr_ts);
        return bbox;
    }

    // Filter can be rendered on an object without a bbox,
    // as long as filter uses `userSpaceOnUse`.
    if let Some(ref id) = g.filter {
        if let Some(filter_node) = node.tree().defs_by_id(id) {
            if let usvg::NodeKind::Filter(ref filter) = *filter_node.borrow() {
                let ts = usvg::Transform::from_native(&curr_ts);
                let background = prepare_filter_background(node, filter, layers.image_size());
                let fill_paint = prepare_filter_fill_paint(node, filter, bbox, ts, &sub_img);
                let stroke_paint = prepare_filter_stroke_paint(node, filter, bbox, ts, &sub_img);
                crate::filter::apply(filter, bbox, &ts, &node.tree(),
                                     background.as_ref(), fill_paint.as_ref(), stroke_paint.as_ref(),
                                     &mut sub_img);
            }
        }
    }

    // Clipping and masking can be done only for objects with a valid bbox.
    if let Some(bbox) = bbox {
        if let Some(ref id) = g.clip_path {
            if let Some(clip_node) = node.tree().defs_by_id(id) {
                if let usvg::NodeKind::ClipPath(ref cp) = *clip_node.borrow() {
                    let mut sub_p = qt::Painter::new(&mut sub_img);
                    sub_p.set_transform(&curr_ts);

                    crate::clip::clip(&clip_node, cp, bbox, layers, &mut sub_p);
                }
            }
        }

        if let Some(ref id) = g.mask {
            if let Some(mask_node) = node.tree().defs_by_id(id) {
                if let usvg::NodeKind::Mask(ref mask) = *mask_node.borrow() {
                    let mut sub_p = qt::Painter::new(&mut sub_img);
                    sub_p.set_transform(&curr_ts);

                    crate::mask::mask(&mask_node, mask, bbox, layers, &mut sub_p);
                }
            }
        }
    }

    if !g.opacity.is_default() {
        p.set_opacity(g.opacity.value());
    }

    let curr_ts = p.get_transform();
    p.set_transform(&qt::Transform::default());

    p.draw_image(0.0, 0.0, &sub_img);

    p.set_opacity(1.0);
    p.set_transform(&curr_ts);

    bbox
}

/// Renders an image used by `BackgroundImage` or `BackgroundAlpha` filter inputs.
fn prepare_filter_background(
    parent: &usvg::Node,
    filter: &usvg::Filter,
    img_size: ScreenSize,
) -> Option<qt::Image> {
    let start_node = parent.filter_background_start_node(filter)?;

    let tree = parent.tree();
    let mut img = create_subimage(img_size)?;
    let view_box = tree.svg_node().view_box;

    let mut painter = qt::Painter::new(&mut img);
    // Render from the `start_node` until the `parent`. The `parent` itself is excluded.
    let mut state = RenderState::RenderUntil(parent.clone());
    render_node_to_canvas(&start_node, view_box, img_size, &mut state, &mut painter);
    painter.end();

    Some(img)
}

/// Renders an image used by `FillPaint`/`StrokePaint` filter input.
///
/// FillPaint/StrokePaint is mostly an undefined behavior and will produce different results
/// in every application.
/// And since there are no expected behaviour, we will simply fill the filter region.
///
/// https://github.com/w3c/fxtf-drafts/issues/323
fn prepare_filter_fill_paint(
    parent: &usvg::Node,
    filter: &usvg::Filter,
    bbox: Option<Rect>,
    ts: usvg::Transform,
    canvas: &qt::Image,
) -> Option<qt::Image> {
    let region = crate::filter::calc_region(filter, bbox, &ts, canvas).ok()?;
    let mut img = create_subimage(region.size())?;
    if let usvg::NodeKind::Group(ref g) = *parent.borrow() {
        if let Some(paint) = g.filter_fill.clone() {
            let mut painter = qt::Painter::new(&mut img);
            let style_bbox = bbox.unwrap_or_else(|| Rect::new(0.0, 0.0, 1.0, 1.0).unwrap());
            let fill = Some(usvg::Fill::from_paint(paint));
            crate::paint_server::fill(&parent.tree(), &fill, style_bbox, &mut painter);
            painter.draw_rect(0.0, 0.0, region.width() as f64, region.height() as f64);
        }
    }

    Some(img)
}

/// The same as `prepare_filter_fill_paint`, but for `StrokePaint`.
fn prepare_filter_stroke_paint(
    parent: &usvg::Node,
    filter: &usvg::Filter,
    bbox: Option<Rect>,
    ts: usvg::Transform,
    canvas: &qt::Image,
) -> Option<qt::Image> {
    let region = crate::filter::calc_region(filter, bbox, &ts, canvas).ok()?;
    let mut img = create_subimage(region.size())?;
    if let usvg::NodeKind::Group(ref g) = *parent.borrow() {
        if let Some(paint) = g.filter_stroke.clone() {
            let mut painter = qt::Painter::new(&mut img);
            let style_bbox = bbox.unwrap_or_else(|| Rect::new(0.0, 0.0, 1.0, 1.0).unwrap());
            let fill = Some(usvg::Fill::from_paint(paint));
            crate::paint_server::fill(&parent.tree(), &fill, style_bbox, &mut painter);
            painter.draw_rect(0.0, 0.0, region.width() as f64, region.height() as f64);
        }
    }

    Some(img)
}

pub(crate) fn create_subimage(size: ScreenSize) -> Option<qt::Image> {
    let image = qt::Image::new_rgba_premultiplied(size.width(), size.height());
    match image {
        Some(mut image) => {
            image.fill(0, 0, 0, 0);
            Some(image)
        }
        None => {
            warn!("Failed to create a {}x{} image.", size.width(), size.height());
            None
        }
    }
}

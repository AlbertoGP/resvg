// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use std::cell::RefCell;
use std::ops::Deref;
use std::rc::Rc;

use usvg::ScreenSize;
use crate::skia;


type LayerData = Rc<RefCell<skia::Surface>>;

/// Stack of image layers.
///
/// Instead of creating a new layer each time we need one,
/// we are reusing an existing one.
pub struct Layers {
    d: Vec<LayerData>,
    /// Use Rc as a shared counter.
    counter: Rc<()>,
    img_size: ScreenSize,
}

impl Layers {
    /// Creates `Layers`.
    pub fn new(img_size: ScreenSize) -> Self {
        Layers {
            d: Vec::new(),
            counter: Rc::new(()),
            img_size,
        }
    }

    /// Returns a layer size.
    pub fn image_size(&self) -> ScreenSize {
        self.img_size
    }

    /// Returns a first free layer to draw on.
    ///
    /// - If there are no free layers - will create a new one.
    /// - If there is a free layer - it will clear it before return.
    /// - If a new layer allocation fail - will return `None`.
    pub fn get(&mut self) -> Option<Layer> {
        let used_layers = Rc::strong_count(&self.counter) - 1;
        if used_layers == self.d.len() {
            match crate::render::create_subsurface(self.img_size) {
                Some(img) => {
                    self.d.push(Rc::new(RefCell::new(img)));
                    Some(Layer {
                        d: self.d[self.d.len() - 1].clone(),
                        _counter_holder: self.counter.clone(),
                    })
                }
                None => {
                    None
                }
            }
        } else {
            {
                let img = self.d[used_layers].clone();
                img.borrow_mut().fill(0, 0, 0, 0);
            }

            Some(Layer {
                d: self.d[used_layers].clone(),
                _counter_holder: self.counter.clone(),
            })
        }
    }
}

impl Drop for Layers {
    fn drop(&mut self) {
        debug_assert!(Rc::strong_count(&self.counter) == 1);
    }
}

/// The layer object.
pub struct Layer {
    d: LayerData,
    // When Layer goes out of scope, Layers::counter will be automatically decreased.
    _counter_holder: Rc<()>,
}

impl Deref for Layer {
    type Target = LayerData;

    fn deref(&self) -> &Self::Target {
        &self.d
    }
}

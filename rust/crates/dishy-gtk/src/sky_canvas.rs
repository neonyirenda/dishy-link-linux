//! The GL sky canvas: one `GLArea` and its `SkyRenderer`, shared by the Sky
//! page and the dashboard's Obstructions card (spec 0187). It owns the
//! renderer's lifecycle, drag and zoom, and the frame tick; what to draw is the
//! caller's.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::sky_render::{SkyRenderer, required_version};

type Hook = RefCell<Option<Box<dyn Fn()>>>;
type Seed = RefCell<Option<Box<dyn Fn(&mut SkyRenderer)>>>;

struct Inner {
    area: gtk::GLArea,
    renderer: RefCell<Option<SkyRenderer>>,
    failed: Cell<bool>,
    /// Gives a freshly created renderer the current survey and layers.
    seed: Seed,
    on_failed: Hook,
    on_frame: Hook,
}

#[derive(Clone)]
pub struct SkyCanvas {
    inner: Rc<Inner>,
}

impl Default for SkyCanvas {
    fn default() -> Self {
        Self::new()
    }
}

impl SkyCanvas {
    pub fn new() -> Self {
        let area = gtk::GLArea::builder()
            .hexpand(true)
            .vexpand(true)
            .auto_render(true)
            .has_depth_buffer(true)
            .build();
        let (major, minor) = required_version();
        area.set_required_version(major, minor);
        let inner = Rc::new(Inner {
            area,
            renderer: RefCell::new(None),
            failed: Cell::new(false),
            seed: RefCell::new(None),
            on_failed: RefCell::new(None),
            on_frame: RefCell::new(None),
        });
        connect_gl(&inner);
        connect_input(&inner);
        start_frame_loop(&inner);
        Self { inner }
    }

    pub fn area(&self) -> &gtk::GLArea {
        &self.inner.area
    }

    /// Run `f` on the renderer, if one exists yet. It is created on the first
    /// frame, so a caller also sets the seed for that moment.
    pub fn with_renderer<R>(&self, f: impl FnOnce(&mut SkyRenderer) -> R) -> Option<R> {
        self.inner.renderer.borrow_mut().as_mut().map(f)
    }

    pub fn set_seed(&self, seed: impl Fn(&mut SkyRenderer) + 'static) {
        *self.inner.seed.borrow_mut() = Some(Box::new(seed));
    }

    /// Called once when GL cannot be used, so the caller can say why.
    pub fn connect_failed(&self, hook: impl Fn() + 'static) {
        *self.inner.on_failed.borrow_mut() = Some(Box::new(hook));
    }

    /// Called on every frame tick while the canvas is on screen.
    pub fn connect_frame(&self, hook: impl Fn() + 'static) {
        *self.inner.on_frame.borrow_mut() = Some(Box::new(hook));
    }

    pub fn is_failed(&self) -> bool {
        self.inner.failed.get()
    }

    pub fn queue_render(&self) {
        self.inner.area.queue_render();
    }
}

fn fail(inner: &Inner) {
    inner.failed.set(true);
    if let Some(hook) = inner.on_failed.borrow().as_ref() {
        hook();
    }
}

fn connect_gl(inner: &Rc<Inner>) {
    let this = Rc::downgrade(inner);
    inner.area.connect_realize(move |area| {
        area.make_current();
        let Some(inner) = this.upgrade() else { return };
        inner.failed.set(false);
        if let Some(error) = area.error() {
            tracing::warn!("sky GL context: {error}");
            fail(&inner);
        }
    });

    let this = Rc::downgrade(inner);
    inner.area.connect_unrealize(move |area| {
        area.make_current();
        if let Some(inner) = this.upgrade() {
            // Drop the renderer while its context is still current; otherwise
            // its GL names would be freed against whatever context comes next.
            inner.renderer.borrow_mut().take();
        }
    });

    let this = Rc::downgrade(inner);
    inner.area.connect_render(move |area, _context| {
        let Some(inner) = this.upgrade() else {
            return glib::Propagation::Proceed;
        };
        if inner.failed.get() {
            return glib::Propagation::Proceed;
        }
        if inner.renderer.borrow().is_none() {
            match SkyRenderer::new() {
                Ok(mut renderer) => {
                    if let Some(seed) = inner.seed.borrow().as_ref() {
                        seed(&mut renderer);
                    }
                    *inner.renderer.borrow_mut() = Some(renderer);
                }
                Err(error) => {
                    tracing::warn!("sky renderer unavailable: {error}");
                    fail(&inner);
                    return glib::Propagation::Proceed;
                }
            }
        }
        let now_ms = glib::monotonic_time() as f64 / 1_000.0;
        let scale = area.scale_factor() as f32;
        if let Some(renderer) = inner.renderer.borrow_mut().as_mut() {
            renderer.render(area.width(), area.height(), scale, now_ms);
        }
        glib::Propagation::Stop
    });
}

fn now_ms() -> f64 {
    glib::monotonic_time() as f64 / 1_000.0
}

fn connect_input(inner: &Rc<Inner>) {
    let drag = gtk::GestureDrag::new();
    let previous = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
    let this = Rc::downgrade(inner);
    let drag_previous = previous.clone();
    drag.connect_drag_begin(move |_, _, _| {
        drag_previous.set((0.0, 0.0));
        if let Some(inner) = this.upgrade()
            && let Some(renderer) = inner.renderer.borrow_mut().as_mut()
        {
            renderer.camera_mut().drag_start(now_ms());
        }
    });
    let this = Rc::downgrade(inner);
    let drag_previous = previous.clone();
    drag.connect_drag_update(move |_, offset_x, offset_y| {
        let (last_x, last_y) = drag_previous.get();
        drag_previous.set((offset_x, offset_y));
        if let Some(inner) = this.upgrade()
            && let Some(renderer) = inner.renderer.borrow_mut().as_mut()
        {
            renderer
                .camera_mut()
                .drag_by(offset_x - last_x, offset_y - last_y, now_ms());
            inner.area.queue_render();
        }
    });
    let this = Rc::downgrade(inner);
    drag.connect_drag_end(move |_, _, _| {
        if let Some(inner) = this.upgrade()
            && let Some(renderer) = inner.renderer.borrow_mut().as_mut()
        {
            renderer.camera_mut().drag_end();
        }
    });
    inner.area.add_controller(drag);

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    let this = Rc::downgrade(inner);
    scroll.connect_scroll(move |_, _dx, dy| {
        if let Some(inner) = this.upgrade()
            && let Some(renderer) = inner.renderer.borrow_mut().as_mut()
        {
            renderer.camera_mut().zoom(dy * 100.0, now_ms());
            inner.area.queue_render();
        }
        glib::Propagation::Stop
    });
    inner.area.add_controller(scroll);
}

/// 60 Hz while on screen only: an unmapped canvas costs nothing.
fn start_frame_loop(inner: &Rc<Inner>) {
    let this = Rc::downgrade(inner);
    glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
        let Some(inner) = this.upgrade() else {
            return glib::ControlFlow::Break;
        };
        if inner.area.is_mapped() {
            inner.area.queue_render();
            if let Some(hook) = inner.on_frame.borrow().as_ref() {
                hook();
            }
        }
        glib::ControlFlow::Continue
    });
}

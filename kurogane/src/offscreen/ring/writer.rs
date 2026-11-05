//! Writes an offscreen browser's frames into its ring.
//!
//! A slot lags the page by a region, what changed since it was last written.
//! A frame copies that region alone. A popup is kept in a layer and drawn over
//! each frame of the view. An OS brings its private device through
//! [`FrameDevice`]; everything here runs on CEF's UI thread.

use std::rc::Rc;
use std::sync::{Arc, Once};

use tracing::warn;

use super::{Claim, Ring, Shape, SlotId};
use crate::offscreen::frame::{PixelFrame, PixelRect, SharedTextureFrame, TextureFormat};

/// Returns the shape of `frame`.
fn shape_of(frame: &SharedTextureFrame<'_>) -> Shape {
    Shape {
        width: frame.width(),
        height: frame.height(),
        format: frame.format(),
    }
}

/// One copy of a frame's work.
pub(in crate::offscreen) enum Copy<'a, D: FrameDevice + ?Sized> {
    /// The part `region` of a frame of CEF's into the same part of a slot.
    Frame {
        from: &'a D::Texture,
        to: &'a D::Slot,
        region: PixelRect,
    },
    /// The top-left `width` by `height` of a popup's frame into its layer.
    Layer {
        from: &'a D::Texture,
        to: &'a D::Texture,
        width: u32,
        height: u32,
    },
    /// The top-left of a layer into the part `place` of a slot.
    Popup {
        from: &'a D::Texture,
        to: &'a D::Slot,
        place: PixelRect,
    },
    /// The part `region` of what was staged into the same part of a slot.
    Staged {
        from: &'a D::Staging,
        to: &'a D::Slot,
        region: PixelRect,
    },
}

/// Private device of an OS.
pub(in crate::offscreen) trait FrameDevice {
    /// What a host opens a slot by.
    type Native;
    /// Slot, open on the device.
    type Slot;
    /// Texture of CEF's or a popup's layer, open on the device.
    type Texture;
    /// What a browser keeps of CEF's textures between frames.
    type Kept: Default;
    /// Memory the CPU writes a software frame to.
    type Staging;

    /// Makes a slot of `shape`.
    fn make_slot(&self, shape: Shape) -> Result<(Self::Native, Self::Slot), String>;

    /// Makes a popup's layer of `shape`.
    fn make_layer(&self, shape: Shape) -> Result<Self::Texture, String>;

    /// Makes staging memory of `shape`.
    fn make_staging(&self, shape: Shape) -> Result<Self::Staging, String>;

    /// Writes the part `region` of `frame` to `staging`.
    fn stage(&self, staging: &mut Self::Staging, frame: &PixelFrame<'_>, region: PixelRect);

    /// Runs `copies` in order and waits for them. The frames they name go back
    /// to CEF when this returns. A lease may read a slot from then on.
    fn run(&self, copies: &[Copy<'_, Self>]) -> Result<(), String>;

    /// Returns whether the device is lost.
    fn is_lost(&self) -> bool;

    /// Returns whether a frame of CEF's textures can be waited on here. Where it
    /// cannot, a ring takes CEF's pixel frames.
    fn waits_on_textures(&self) -> bool {
        true
    }

    /// Returns a device in place of this lost one.
    fn reopen(&self) -> Result<Rc<Self>, String>;
}

/// [`FrameDevice`] that opens CEF's textures.
pub(in crate::offscreen) trait OpensFrames: FrameDevice {
    /// Returns the texture of `frame`, of `shape`.
    fn open(
        &self,
        kept: &mut Self::Kept,
        frame: &SharedTextureFrame<'_>,
        shape: Shape,
    ) -> Result<Self::Texture, String>;
}

/// Returns the smallest rectangle holding `a` and `b`.
fn join(a: PixelRect, b: PixelRect) -> PixelRect {
    if a.width == 0 || a.height == 0 {
        return b;
    }
    if b.width == 0 || b.height == 0 {
        return a;
    }
    let (left, top) = (a.x.min(b.x), a.y.min(b.y));
    let right = (a.x + a.width).max(b.x + b.width);
    let bottom = (a.y + a.height).max(b.y + b.height);
    PixelRect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    }
}

/// Returns the part of `rect` inside a frame of `shape`, empty when none is.
fn clip(rect: PixelRect, shape: Shape) -> PixelRect {
    let x = rect.x.min(shape.width);
    let y = rect.y.min(shape.height);
    PixelRect {
        x,
        y,
        width: rect.width.min(shape.width - x),
        height: rect.height.min(shape.height - y),
    }
}

/// Returns the rectangle of a whole frame of `shape`.
fn whole(shape: Shape) -> PixelRect {
    PixelRect {
        x: 0,
        y: 0,
        width: shape.width,
        height: shape.height,
    }
}

/// Returns whether `rect` holds no pixel.
fn is_empty(rect: PixelRect) -> bool {
    rect.width == 0 || rect.height == 0
}

/// Slot open on the device, with the region it lags the page by.
struct Open<D: FrameDevice> {
    id: SlotId,
    slot: D::Slot,
    behind: PixelRect,
}

/// A frame of the view as CEF describes it.
#[derive(Clone, Copy, Debug)]
pub(in crate::offscreen) struct Incoming {
    pub(in crate::offscreen) shape: Shape,
    /// What changed since the frame of the previous capture
    pub(in crate::offscreen) damage: PixelRect,
    /// Capture number; one past the last means nothing went missing between
    pub(in crate::offscreen) counter: Option<u64>,
    /// Part of the frame that holds the page
    pub(in crate::offscreen) valid: PixelRect,
}

/// Popup's layer over the view.
struct Layer<D: FrameDevice> {
    texture: D::Texture,
    shape: Shape,
    /// CEF's textures of the popup
    kept: D::Kept,
}

/// Staging memory of the newest software frames.
struct Staged<D: FrameDevice> {
    staging: D::Staging,
    shape: Shape,
}

/// Writes one browser's frames into its ring. UI thread alone.
pub(in crate::offscreen) struct Writer<D: FrameDevice> {
    device: Rc<D>,
    ring: Arc<Ring<D::Native>>,
    slots: Vec<Open<D>>,
    kept: D::Kept,
    shape: Option<Shape>,
    counter: Option<u64>,
    layer: Option<Layer<D>>,
    /// Popup's place while it shows
    place: Option<PixelRect>,
    staged: Option<Staged<D>>,
    /// Failure of a software frame, whose callback returns nothing
    failure: Option<String>,
    /// A frame was lost that the page may never repeat
    wants_view: bool,
}

impl<D: FrameDevice> Writer<D> {
    /// Writer of `ring` on `device`.
    pub(in crate::offscreen) fn new(device: Rc<D>, ring: Arc<Ring<D::Native>>) -> Self {
        Self {
            device,
            ring,
            slots: Vec::new(),
            kept: D::Kept::default(),
            shape: None,
            counter: None,
            layer: None,
            place: None,
            staged: None,
            failure: None,
            wants_view: false,
        }
    }

    /// Returns whether the ring takes CEF's textures, else its pixel frames.
    pub(in crate::offscreen) fn takes_textures(&self) -> bool {
        self.device.waits_on_textures()
    }

    /// Answers `problem` with a new device when this one is lost, which leaves
    /// the ring without slots. Hands `problem` back otherwise.
    fn recover(&mut self, problem: String) -> Result<(), String> {
        if !self.device.is_lost() {
            return Err(problem);
        }
        warn!("an offscreen browser's private device was lost: {problem}");
        self.device = self.device.reopen()?;
        self.ring.retire_slots();
        self.slots.clear();
        self.kept = D::Kept::default();
        self.counter = None;
        self.layer = None;
        self.staged = None;
        self.wants_view = true;
        Ok(())
    }

    /// Takes whether the ring needs a frame of the view it has no other way to
    /// get.
    pub(in crate::offscreen) fn take_wants_view(&mut self) -> bool {
        std::mem::take(&mut self.wants_view)
    }

    /// Notes frames of `shape`, whose first drops what the old shape kept.
    fn reshape(&mut self, shape: Shape) {
        if self.shape != Some(shape) {
            self.shape = Some(shape);
            self.kept = D::Kept::default();
            self.counter = None;
        }
    }

    /// Notes that the page changed in `region`.
    fn fall_behind(&mut self, region: PixelRect) {
        for slot in &mut self.slots {
            slot.behind = join(slot.behind, region);
        }
    }

    /// Returns the index of a slot for a frame of `shape`, None when leases
    /// hold them all.
    fn claim(&mut self, shape: Shape) -> Result<Option<usize>, String> {
        let id = match self.ring.claim(shape) {
            Claim::Slot(id) => id,
            Claim::Make => {
                let ring = &self.ring;
                self.slots.retain(|open| ring.holds(open.id));
                let (native, slot) = self.device.make_slot(shape)?;
                let id = self.ring.add(shape, native);
                self.slots.push(Open {
                    id,
                    slot,
                    behind: whole(shape),
                });
                return Ok(Some(self.slots.len() - 1));
            }
            Claim::Full => {
                static WARNED: Once = Once::new();
                WARNED.call_once(|| {
                    warn!("an offscreen browser's leases hold every slot; it drops frames");
                });
                return Ok(None);
            }
        };
        self.slots
            .iter()
            .position(|open| open.id == id)
            .map(Some)
            .ok_or_else(|| "a slot of the ring is not open".to_owned())
    }

    /// Writes a frame of the view into a slot and publishes it. `open` gives its
    /// texture.
    pub(in crate::offscreen) fn view(
        &mut self,
        incoming: Incoming,
        open: impl FnOnce(&D, &mut D::Kept) -> Result<D::Texture, String>,
    ) -> Result<(), String> {
        let Incoming {
            shape,
            damage,
            counter,
            valid,
        } = incoming;
        self.reshape(shape);
        let consecutive =
            counter.is_some() && counter == self.counter.and_then(|last| last.checked_add(1));
        self.counter = counter;
        let source = match open(&self.device, &mut self.kept) {
            Ok(source) => source,
            Err(problem) => return self.recover(problem),
        };
        self.fall_behind(if consecutive {
            clip(damage, shape)
        } else {
            whole(shape)
        });
        let index = match self.claim(shape) {
            Ok(Some(index)) => index,
            Ok(None) => return Ok(()),
            Err(problem) => return self.recover(problem),
        };

        let open_slot = &self.slots[index];
        let id = open_slot.id;
        let mut copies = Vec::with_capacity(2);
        if !is_empty(open_slot.behind) {
            copies.push(Copy::Frame {
                from: &source,
                to: &open_slot.slot,
                region: open_slot.behind,
            });
        }
        let over = self.place.zip(self.layer.as_ref()).map(|(place, layer)| {
            let size = PixelRect {
                x: place.x,
                y: place.y,
                width: place.width.min(layer.shape.width),
                height: place.height.min(layer.shape.height),
            };
            (clip(size, shape), &layer.texture)
        });
        let mut drawn = PixelRect::default();
        if let Some((place, from)) = over.filter(|(place, _)| !is_empty(*place)) {
            drawn = place;
            copies.push(Copy::Popup {
                from,
                to: &open_slot.slot,
                place,
            });
        }
        let copied = self.device.run(&copies);
        drop(copies);
        if let Err(problem) = copied {
            self.ring.abandon(id);
            return self.recover(problem);
        }

        // The slot differs from the page only under the popup
        self.slots[index].behind = drawn;
        self.ring.publish(id, valid);
        Ok(())
    }

    /// Keeps a frame of the popup in its layer. A frame of the view brings it
    /// to a slot. `open` gives its texture.
    pub(in crate::offscreen) fn popup(
        &mut self,
        shape: Shape,
        open: impl FnOnce(&D, &mut D::Kept) -> Result<D::Texture, String>,
    ) -> Result<(), String> {
        if self.layer.as_ref().is_none_or(|layer| layer.shape != shape) {
            self.layer = Some(Layer {
                texture: self.device.make_layer(shape)?,
                shape,
                kept: D::Kept::default(),
            });
        }
        let Some(layer) = self.layer.as_mut() else {
            return Ok(());
        };
        let source = open(&self.device, &mut layer.kept)?;
        self.device.run(&[Copy::Layer {
            from: &source,
            to: &layer.texture,
            width: shape.width,
            height: shape.height,
        }])
    }

    /// Places the popup at `place`, None for no popup. Returns whether slots
    /// hold something a fresh frame of the view must replace.
    pub(in crate::offscreen) fn place_popup(&mut self, place: Option<PixelRect>) -> bool {
        let old = std::mem::replace(&mut self.place, place);
        if place.is_none() {
            self.layer = None;
        }
        if old == place {
            return false;
        }
        if let Some(old) = old {
            self.fall_behind(old);
        }
        true
    }

    /// Writes a frame of pixels into a slot and publishes it, for Chromium
    /// compositing in software.
    pub(in crate::offscreen) fn pixels(&mut self, frame: &PixelFrame<'_>) {
        if let Err(problem) = self.upload(frame) {
            self.failure = Some(problem);
        }
    }

    /// Takes the failure of a software frame, the ring's end.
    pub(in crate::offscreen) fn take_failure(&mut self) -> Option<String> {
        self.failure.take()
    }

    fn upload(&mut self, frame: &PixelFrame<'_>) -> Result<(), String> {
        let shape = Shape {
            width: frame.width(),
            height: frame.height(),
            format: TextureFormat::Bgra8,
        };
        self.reshape(shape);
        let damage = frame
            .damage()
            .iter()
            .copied()
            .fold(PixelRect::default(), join);
        self.fall_behind(clip(damage, shape));
        let Some(index) = self.claim(shape)? else {
            return Ok(());
        };
        if self
            .staged
            .as_ref()
            .is_none_or(|staged| staged.shape != shape)
        {
            self.staged = Some(Staged {
                staging: self.device.make_staging(shape)?,
                shape,
            });
        }
        let Some(staged) = self.staged.as_mut() else {
            return Ok(());
        };

        let open_slot = &mut self.slots[index];
        let region = open_slot.behind;
        let id = open_slot.id;
        self.device.stage(&mut staged.staging, frame, region);
        if !is_empty(region) {
            let copied = self.device.run(&[Copy::Staged {
                from: &staged.staging,
                to: &open_slot.slot,
                region,
            }]);
            if let Err(problem) = copied {
                self.ring.abandon(id);
                return Err(problem);
            }
        }
        open_slot.behind = PixelRect::default();
        self.ring.publish(id, whole(shape));
        Ok(())
    }
}

impl<D: OpensFrames> Writer<D> {
    /// Writes a frame of the view in CEF's texture, changed in `damage`.
    pub(in crate::offscreen) fn view_frame(
        &mut self,
        frame: &SharedTextureFrame<'_>,
        damage: PixelRect,
    ) -> Result<(), String> {
        let shape = shape_of(frame);
        let incoming = Incoming {
            shape,
            damage,
            counter: frame.capture_counter(),
            valid: frame.content_rect(),
        };
        self.view(incoming, |device, kept| device.open(kept, frame, shape))
    }

    /// Keeps a frame of the popup in CEF's texture.
    pub(in crate::offscreen) fn popup_frame(
        &mut self,
        frame: &SharedTextureFrame<'_>,
    ) -> Result<(), String> {
        let shape = shape_of(frame);
        self.popup(shape, |device, kept| device.open(kept, frame, shape))
    }
}

// Written out since the device's objects have no Debug worth printing
impl<D: FrameDevice> std::fmt::Debug for Writer<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Writer")
            .field("slots", &self.slots.len())
            .field("shape", &self.shape)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::Mutex;

    use proptest::prelude::*;

    use super::*;

    const SHAPE: Shape = Shape {
        width: 32,
        height: 24,
        format: TextureFormat::Bgra8,
    };

    /// Pixels by place, a texture of the fake device.
    #[derive(Clone, Debug, PartialEq)]
    struct Image {
        width: u32,
        height: u32,
        pixels: Vec<u32>,
    }

    impl Image {
        fn new(shape: Shape, value: u32) -> Self {
            Self {
                width: shape.width,
                height: shape.height,
                pixels: vec![value; (shape.width * shape.height) as usize],
            }
        }

        fn at(&self, x: u32, y: u32) -> u32 {
            self.pixels[(y * self.width + x) as usize]
        }

        fn put(&mut self, x: u32, y: u32, value: u32) {
            self.pixels[(y * self.width + x) as usize] = value;
        }

        fn fill(&mut self, rect: PixelRect, value: u32) {
            for y in rect.y..rect.y + rect.height {
                for x in rect.x..rect.x + rect.width {
                    self.put(x, y, value);
                }
            }
        }
    }

    type Shared = Arc<Mutex<Image>>;

    /// Device that copies between images in memory, counting what it copies.
    #[derive(Default)]
    struct Fake {
        copied: RefCell<u64>,
        fail: RefCell<bool>,
        lost: RefCell<bool>,
    }

    impl FrameDevice for Fake {
        type Native = Shared;
        type Slot = Shared;
        type Texture = Shared;
        type Kept = ();
        type Staging = Image;

        fn make_slot(&self, shape: Shape) -> Result<(Shared, Shared), String> {
            // Fresh memory holds nothing of the page
            let image = Arc::new(Mutex::new(Image::new(shape, 0xdead)));
            Ok((Arc::clone(&image), image))
        }

        fn make_layer(&self, shape: Shape) -> Result<Shared, String> {
            Ok(Arc::new(Mutex::new(Image::new(shape, 0xdead))))
        }

        fn make_staging(&self, shape: Shape) -> Result<Image, String> {
            Ok(Image::new(shape, 0xdead))
        }

        fn stage(&self, staging: &mut Image, frame: &PixelFrame<'_>, region: PixelRect) {
            for y in region.y..region.y + region.height {
                for x in region.x..region.x + region.width {
                    let at = (y * frame.width() + x) as usize * 4;
                    let bytes = frame.bytes()[at..at + 4].try_into().expect("four bytes");
                    staging.put(x, y, u32::from_le_bytes(bytes));
                }
            }
        }

        fn run(&self, copies: &[Copy<'_, Self>]) -> Result<(), String> {
            if *self.fail.borrow() {
                return Err("lost".to_owned());
            }
            for copy in copies {
                match copy {
                    Copy::Frame { from, to, region } => {
                        let from = from.lock().expect("an image");
                        let mut to = to.lock().expect("an image");
                        for y in region.y..region.y + region.height {
                            for x in region.x..region.x + region.width {
                                to.put(x, y, from.at(x, y));
                            }
                        }
                        *self.copied.borrow_mut() += u64::from(region.width * region.height);
                    }
                    Copy::Popup { from, to, place } => {
                        let from = from.lock().expect("an image");
                        let mut to = to.lock().expect("an image");
                        for y in 0..place.height {
                            for x in 0..place.width {
                                to.put(place.x + x, place.y + y, from.at(x, y));
                            }
                        }
                    }
                    Copy::Layer {
                        from,
                        to,
                        width,
                        height,
                    } => {
                        let from = from.lock().expect("an image");
                        let mut to = to.lock().expect("an image");
                        for y in 0..*height {
                            for x in 0..*width {
                                to.put(x, y, from.at(x, y));
                            }
                        }
                    }
                    Copy::Staged { from, to, region } => {
                        let mut to = to.lock().expect("an image");
                        for y in region.y..region.y + region.height {
                            for x in region.x..region.x + region.width {
                                to.put(x, y, from.at(x, y));
                            }
                        }
                    }
                }
            }
            Ok(())
        }

        fn is_lost(&self) -> bool {
            *self.lost.borrow()
        }

        fn reopen(&self) -> Result<Rc<Self>, String> {
            Ok(Rc::new(Self::default()))
        }
    }

    /// A page, the writer of its frames and what a host holds.
    struct Lab {
        page: Image,
        counter: u64,
        device: Rc<Fake>,
        ring: Arc<Ring<Shared>>,
        writer: Writer<Fake>,
    }

    impl Lab {
        fn new() -> Self {
            let device = Rc::new(Fake::default());
            let ring = Arc::new(Ring::new());
            let writer = Writer::new(Rc::clone(&device), Arc::clone(&ring));
            Self {
                page: Image::new(SHAPE, 1),
                counter: 0,
                device,
                ring,
                writer,
            }
        }

        /// Delivers the page as it stands, changed in `damage`.
        fn deliver(&mut self, damage: PixelRect) {
            self.counter += 1;
            let frame = Arc::new(Mutex::new(self.page.clone()));
            let incoming = Incoming {
                shape: SHAPE,
                damage,
                counter: Some(self.counter),
                valid: whole(SHAPE),
            };
            self.writer
                .view(incoming, |_, ()| Ok(frame))
                .expect("a written frame");
        }

        /// Paints `value` over `rect` of the page and delivers it.
        fn paint(&mut self, rect: PixelRect, value: u32) {
            self.page.fill(rect, value);
            self.deliver(rect);
        }

        /// Returns what a lease of the newest frame reads.
        fn leased(&self) -> Option<Image> {
            let taken = self.ring.take()?;
            let image = taken.native.lock().expect("an image").clone();
            self.ring.give_back(taken.id);
            Some(image)
        }

        fn copied(&self) -> u64 {
            *self.device.copied.borrow()
        }
    }

    fn rect(x: u32, y: u32, width: u32, height: u32) -> PixelRect {
        PixelRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_small_change_copies_a_small_region() {
        let mut lab = Lab::new();
        lab.paint(rect(0, 0, 32, 24), 2);
        // Three slots come up to date, each lagging by a small change
        for value in 3..6 {
            lab.paint(rect(1, 1, 2, 2), value);
        }
        let before = lab.copied();
        lab.paint(rect(4, 4, 2, 2), 6);
        lab.leased();
        lab.paint(rect(4, 4, 2, 2), 7);
        let per_frame = (lab.copied() - before) / 2;
        assert!(per_frame < 20, "{per_frame} pixels a frame");
        assert_eq!(lab.leased(), Some(lab.page.clone()));
    }

    #[test]
    fn a_skipped_capture_copies_the_whole_frame() {
        let mut lab = Lab::new();
        lab.paint(rect(0, 0, 32, 24), 2);
        lab.page.fill(rect(10, 10, 5, 5), 9);
        // The frame of that change never came
        lab.counter += 1;
        lab.paint(rect(0, 0, 1, 1), 3);
        assert_eq!(lab.leased(), Some(lab.page.clone()));
    }

    #[test]
    fn a_popup_draws_over_each_frame_and_leaves_with_its_pixels() {
        let mut lab = Lab::new();
        lab.paint(rect(0, 0, 32, 24), 2);
        let popup = Shape {
            width: 4,
            height: 4,
            format: TextureFormat::Bgra8,
        };
        let frame = Arc::new(Mutex::new(Image::new(popup, 7)));
        let place = rect(10, 10, 4, 4);
        lab.writer.place_popup(Some(place));
        lab.writer
            .popup(popup, |_, ()| Ok(frame))
            .expect("a kept popup");
        lab.deliver(whole(SHAPE));

        let mut shown = lab.page.clone();
        shown.fill(place, 7);
        assert_eq!(lab.leased(), Some(shown.clone()));
        lab.paint(rect(0, 0, 1, 1), 3);
        shown.fill(rect(0, 0, 1, 1), 3);
        assert_eq!(lab.leased(), Some(shown));

        assert!(lab.writer.place_popup(None));
        lab.deliver(rect(0, 0, 1, 1));
        lab.deliver(rect(0, 0, 1, 1));
        lab.deliver(rect(0, 0, 1, 1));
        assert_eq!(lab.leased(), Some(lab.page.clone()));
    }

    #[test]
    fn a_failed_copy_frees_its_slot_and_ends_the_frame() {
        let mut lab = Lab::new();
        lab.paint(rect(0, 0, 32, 24), 2);
        *lab.device.fail.borrow_mut() = true;
        let frame = Arc::new(Mutex::new(lab.page.clone()));
        let incoming = Incoming {
            shape: SHAPE,
            damage: whole(SHAPE),
            counter: Some(99),
            valid: whole(SHAPE),
        };
        assert!(lab.writer.view(incoming, |_, ()| Ok(frame)).is_err());
    }

    #[test]
    fn a_lost_device_gives_way_to_a_new_one_and_new_slots() {
        let mut lab = Lab::new();
        lab.paint(rect(0, 0, 32, 24), 2);
        let held = lab.ring.take().expect("a frame");

        *lab.device.fail.borrow_mut() = true;
        *lab.device.lost.borrow_mut() = true;
        lab.paint(rect(1, 1, 2, 2), 3);
        assert!(lab.writer.take_wants_view());
        assert!(lab.ring.take().is_none());

        // The next frame fills a slot of a new name on the new device
        lab.paint(rect(4, 4, 2, 2), 4);
        let taken = lab.ring.take().expect("a frame");
        assert_ne!(taken.id, held.id);
        assert_eq!(&*taken.native.lock().expect("an image"), &lab.page);
        assert!(!lab.ring.give_back(taken.id));

        // A lease of the old device ends without troubling the ring
        lab.ring.give_back(held.id);
        assert!(!lab.ring.holds(held.id));
    }

    #[test]
    fn software_frames_keep_a_slot_whole_from_their_damage() {
        let mut lab = Lab::new();
        let mut bytes = vec![0_u8; (SHAPE.width * SHAPE.height * 4) as usize];
        let mut send = |lab: &mut Lab, damage: PixelRect, value: u8| {
            for y in damage.y..damage.y + damage.height {
                for x in damage.x..damage.x + damage.width {
                    let at = ((y * SHAPE.width + x) * 4) as usize;
                    bytes[at..at + 4].copy_from_slice(&[value, 0, 0, 0]);
                    lab.page.put(x, y, u32::from(value));
                }
            }
            let damages = [damage];
            let frame = PixelFrame {
                bytes: &bytes,
                width: SHAPE.width,
                height: SHAPE.height,
                damage: &damages,
            };
            lab.writer.pixels(&frame);
        };
        send(&mut lab, whole(SHAPE), 2);
        send(&mut lab, rect(2, 2, 3, 3), 3);
        send(&mut lab, rect(8, 8, 3, 3), 4);
        send(&mut lab, rect(2, 8, 3, 3), 5);
        assert_eq!(lab.writer.take_failure(), None);
        assert_eq!(lab.leased(), Some(lab.page.clone()));
    }

    /// What can happen between two frames of a page.
    #[derive(Clone, Debug)]
    enum Step {
        Paint {
            x: u32,
            y: u32,
            width: u32,
            height: u32,
            value: u32,
        },
        /// A capture that never arrived, its change lost with it
        Skip {
            x: u32,
            y: u32,
            width: u32,
            height: u32,
            value: u32,
        },
        Lease,
        Hold,
        Release,
    }

    fn step() -> impl Strategy<Value = Step> {
        let area = (
            0..SHAPE.width,
            0..SHAPE.height,
            1..12_u32,
            1..12_u32,
            2..1000_u32,
        );
        prop_oneof![
            6 => area.clone().prop_map(|(x, y, width, height, value)| Step::Paint {
                x, y, width: width.min(SHAPE.width - x), height: height.min(SHAPE.height - y), value,
            }),
            1 => area.prop_map(|(x, y, width, height, value)| Step::Skip {
                x, y, width: width.min(SHAPE.width - x), height: height.min(SHAPE.height - y), value,
            }),
            2 => Just(Step::Lease),
            1 => Just(Step::Hold),
            1 => Just(Step::Release),
        ]
    }

    proptest! {
        // Damage-only copies leave every published frame equal to the page,
        // whatever a host holds and whichever captures go missing
        #[test]
        fn a_leased_frame_is_always_the_page(steps in proptest::collection::vec(step(), 1..200)) {
            let mut lab = Lab::new();
            lab.paint(rect(0, 0, SHAPE.width, SHAPE.height), 1);
            let mut delivered = lab.page.clone();
            let mut held = Vec::new();
            for step in steps {
                match step {
                    Step::Paint { x, y, width, height, value } => {
                        lab.paint(rect(x, y, width, height), value);
                        delivered = lab.page.clone();
                    }
                    Step::Skip { x, y, width, height, value } => {
                        lab.page.fill(rect(x, y, width, height), value);
                        lab.counter += 1;
                    }
                    Step::Lease => {
                        if let Some(image) = lab.leased() {
                            prop_assert_eq!(&image, &delivered);
                        }
                    }
                    Step::Hold => {
                        if held.len() < 2 && let Some(taken) = lab.ring.take() {
                            held.push(taken);
                        }
                    }
                    Step::Release => {
                        if let Some(taken) = held.pop() {
                            lab.ring.give_back(taken.id);
                        }
                    }
                }
            }
            // Whatever came before, a delivered frame ends equal to the page
            lab.paint(rect(0, 0, 1, 1), 2000);
            let settled = lab.ring.take();
            prop_assert!(settled.is_some());
            if let Some(taken) = settled {
                prop_assert_eq!(&*taken.native.lock().expect("an image"), &lab.page);
            }
        }
    }
}

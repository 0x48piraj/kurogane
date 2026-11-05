//! Frame ring of an offscreen browser.
//!
//! Each frame is copied into a slot inside CEF's callback. The host leases the
//! newest slot and reads it as long as it likes; the slot returns when the lease
//! ends. A leased slot is never written. A host imports each slot once by its
//! name.

#[cfg_attr(windows, path = "d3d.rs")]
#[cfg_attr(target_os = "macos", path = "metal.rs")]
#[cfg_attr(target_os = "linux", path = "vulkan.rs")]
mod os;
mod writer;

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use tracing::warn;

use super::frame::{PixelRect, TextureFormat};
use crate::chromium_flags::ChromiumFlags;

/// Writes a browser's frames into its ring on this OS's private device.
pub(super) type Writer = writer::Writer<os::Device>;

/// What a host opens a slot by on this OS.
#[cfg(windows)]
type Native = std::os::windows::io::OwnedHandle;
#[cfg(not(windows))]
type Native = os::Native;

/// Slots of a browser on this OS.
pub(super) type NativeRing = Ring<Native>;

/// Returns the writer of `ring` on `adapter`.
pub(super) fn writer(ring: Arc<NativeRing>, adapter: GpuAdapter) -> Result<Writer, String> {
    Ok(writer::Writer::new(os::Device::shared(adapter)?, ring))
}

/// Slots of one size a ring holds.
const SLOTS: usize = 3;

/// GPU Chromium and the frame ring run on, for
/// [`App::gpu_frames`](crate::App::gpu_frames).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum GpuChoice {
    /// The system's high-performance GPU.
    #[default]
    HighPerformance,
    /// The adapter a host's own device runs on.
    Adapter(GpuAdapter),
}

/// GPU adapter, by the name its OS gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GpuAdapter(os::AdapterId);

impl GpuAdapter {
    /// Windows adapter whose LUID has the parts `high` and `low`.
    #[cfg(windows)]
    pub fn from_luid(high: i32, low: u32) -> Self {
        Self((high, low))
    }

    /// High and low parts of a Windows adapter's LUID.
    #[cfg(windows)]
    pub fn luid(&self) -> (i32, u32) {
        self.0
    }

    /// macOS GPU of this registry id.
    #[cfg(target_os = "macos")]
    pub fn from_registry_id(id: u64) -> Self {
        Self(id)
    }

    /// Registry id of a macOS GPU.
    #[cfg(target_os = "macos")]
    pub fn registry_id(&self) -> u64 {
        self.0
    }

    /// Linux GPU of this DRM render node, major and minor.
    #[cfg(target_os = "linux")]
    pub fn from_render_node(major: u32, minor: u32) -> Self {
        Self((major, minor))
    }

    /// DRM render node of a Linux GPU, major and minor.
    #[cfg(target_os = "linux")]
    pub fn render_node(&self) -> (u32, u32) {
        self.0
    }
}

/// Returns the adapter of `choice`, found once for the process.
pub(crate) fn adapter(choice: GpuChoice) -> Result<GpuAdapter, String> {
    static ADAPTER: OnceLock<Result<GpuAdapter, String>> = OnceLock::new();
    ADAPTER.get_or_init(|| os::adapter(choice)).clone()
}

/// Pins Chromium's GPU process to the adapter of `choice`.
pub(crate) fn apply_gpu_frames_flags(flags: &mut ChromiumFlags, choice: Option<GpuChoice>) {
    let Some(choice) = choice else {
        return;
    };
    match adapter(choice).map(os::pin_switch) {
        Ok(Some((switch, value))) => flags.set_with_value(switch, value),
        Ok(None) => {}
        Err(problem) => warn!("no GPU to pin Chromium to: {problem}"),
    }
}

/// Name of a slot, never given twice. A host imports a slot once per name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SlotId(u64);

/// Size and format every slot of a frame shares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::offscreen) struct Shape {
    pub(in crate::offscreen) width: u32,
    pub(in crate::offscreen) height: u32,
    pub(in crate::offscreen) format: TextureFormat,
}

/// What a slot is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Free,
    Writing,
    /// Holds a frame no lease has taken
    Ready,
    Leased,
}

/// Slot as the ring tracks it. `N` is its OS handle.
#[derive(Debug)]
struct Slot<N> {
    id: SlotId,
    shape: Shape,
    stage: Stage,
    /// Order of the frame it holds among all published
    number: u64,
    /// Part of the slot that holds the page
    valid: PixelRect,
    /// Its device is gone; it leaves the ring when its lease ends
    retired: bool,
    native: Arc<N>,
}

/// Where the next frame goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Claim {
    /// This slot, now being written.
    Slot(SlotId),
    /// A new slot, for [`Ring::add`].
    Make,
    /// Nowhere; leases hold every slot.
    Full,
}

/// Slot a lease holds.
#[derive(Debug)]
struct Taken<N> {
    id: SlotId,
    shape: Shape,
    valid: PixelRect,
    native: Arc<N>,
}

#[derive(Debug)]
struct State<N> {
    slots: Vec<Slot<N>>,
    /// Shape of the newest frame; a slot of another leaves the ring when free
    shape: Option<Shape>,
    next_id: u64,
    next_number: u64,
    /// A frame found every slot leased
    starved: bool,
}

/// Slots of one browser, shared by CEF's UI thread and the host's threads. The
/// lock is held for a change of state only, never across a GPU wait.
#[derive(Debug)]
pub(super) struct Ring<N> {
    state: Mutex<State<N>>,
}

impl<N> Ring<N> {
    /// Ring without slots.
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(State {
                slots: Vec::new(),
                shape: None,
                next_id: 0,
                next_number: 0,
                starved: false,
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State<N>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Claims a slot for a frame of `shape`. A free slot goes first, then a
    /// new one, which keeps the newest frame leasable during the write. The
    /// oldest frame no lease took goes last.
    pub(super) fn claim(&self, shape: Shape) -> Claim {
        let mut state = self.state();
        if state.shape != Some(shape) {
            state.shape = Some(shape);
            state
                .slots
                .retain(|slot| slot.shape == shape || slot.stage == Stage::Leased);
        }
        let of_shape = |slot: &&mut Slot<N>| slot.shape == shape && !slot.retired;
        let count = state.slots.iter_mut().filter(of_shape).count();
        let free = state
            .slots
            .iter_mut()
            .filter(of_shape)
            .find(|slot| slot.stage == Stage::Free);
        let slot = match free {
            Some(slot) => slot,
            None if count < SLOTS => return Claim::Make,
            None => {
                let oldest = state
                    .slots
                    .iter_mut()
                    .filter(of_shape)
                    .filter(|slot| slot.stage == Stage::Ready)
                    .min_by_key(|slot| slot.number);
                match oldest {
                    Some(slot) => slot,
                    None => {
                        state.starved = true;
                        return Claim::Full;
                    }
                }
            }
        };
        slot.stage = Stage::Writing;
        Claim::Slot(slot.id)
    }

    /// Adds a slot of `shape` the writer made for a [`Claim::Make`], being
    /// written. Returns its name.
    pub(super) fn add(&self, shape: Shape, native: N) -> SlotId {
        let mut state = self.state();
        let id = SlotId(state.next_id);
        state.next_id += 1;
        state.slots.push(Slot {
            id,
            shape,
            stage: Stage::Writing,
            number: 0,
            valid: PixelRect::default(),
            retired: false,
            native: Arc::new(native),
        });
        id
    }

    /// Returns whether the slot `id` is still in the ring.
    pub(super) fn holds(&self, id: SlotId) -> bool {
        self.state().slots.iter().any(|slot| slot.id == id)
    }

    /// Publishes the frame just written to `id`, the page in its `valid` part.
    pub(super) fn publish(&self, id: SlotId, valid: PixelRect) {
        let mut state = self.state();
        let number = state.next_number;
        state.next_number += 1;
        if let Some(slot) = state.slots.iter_mut().find(|slot| slot.id == id) {
            slot.stage = Stage::Ready;
            slot.number = number;
            slot.valid = valid;
        }
    }

    /// Takes every slot out of the ring, a lost device's. A leased one goes when
    /// its lease ends.
    pub(super) fn retire_slots(&self) {
        let mut state = self.state();
        state.slots.retain(|slot| slot.stage == Stage::Leased);
        for slot in &mut state.slots {
            slot.retired = true;
        }
    }

    /// Frees `id`, whose write failed.
    pub(super) fn abandon(&self, id: SlotId) {
        if let Some(slot) = self.state().slots.iter_mut().find(|slot| slot.id == id) {
            slot.stage = Stage::Free;
        }
    }

    /// Leases the newest frame and frees the older ones.
    fn take(&self) -> Option<Taken<N>> {
        let mut state = self.state();
        let newest = state
            .slots
            .iter()
            .filter(|slot| slot.stage == Stage::Ready)
            .map(|slot| slot.number)
            .max()?;
        let mut taken = None;
        for slot in &mut state.slots {
            if slot.stage != Stage::Ready {
                continue;
            }
            if slot.number == newest {
                slot.stage = Stage::Leased;
                taken = Some(Taken {
                    id: slot.id,
                    shape: slot.shape,
                    valid: slot.valid,
                    native: Arc::clone(&slot.native),
                });
            } else {
                slot.stage = Stage::Free;
            }
        }
        taken
    }

    /// Ends the lease of `id`. A slot of an older shape leaves the ring.
    /// Returns whether a frame was lost for want of a slot, which a fresh one
    /// makes up for.
    fn give_back(&self, id: SlotId) -> bool {
        let mut state = self.state();
        let current = state.shape;
        if let Some(index) = state.slots.iter().position(|slot| slot.id == id) {
            let slot = &mut state.slots[index];
            if Some(slot.shape) == current && !slot.retired {
                slot.stage = Stage::Free;
            } else {
                state.slots.swap_remove(index);
            }
        }
        std::mem::take(&mut state.starved)
    }
}

/// Frames of an offscreen browser as slots of a ring, from
/// [`OffscreenBrowser::gpu_frames`](crate::OffscreenBrowser::gpu_frames).
/// Clones share the ring.
#[derive(Clone)]
pub struct GpuFrames {
    ring: Arc<NativeRing>,
    adapter: GpuAdapter,
    refresh: Refresh,
}

/// Asks the browser for a fresh frame.
pub(super) type Refresh = Arc<dyn Fn() + Send + Sync>;

// Written out since the refresh has no Debug
impl fmt::Debug for GpuFrames {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GpuFrames")
            .field("adapter", &self.adapter)
            .finish_non_exhaustive()
    }
}

impl GpuFrames {
    /// Frames of `ring` on `adapter`, `refresh` asking for a frame lost for want
    /// of a slot.
    pub(super) fn new(ring: Arc<NativeRing>, adapter: GpuAdapter, refresh: Refresh) -> Self {
        Self {
            ring,
            adapter,
            refresh,
        }
    }

    /// GPU the ring's slots live on, the only one that can open them.
    pub fn adapter(&self) -> GpuAdapter {
        self.adapter
    }

    /// Leases the newest frame not yet leased, None when no new one waits.
    ///
    /// Older frames go back to the ring. A host keeps the lease of the frame it
    /// shows until a newer one comes. Leases past one take slots from the
    /// frames to come, which are dropped once leases hold every slot.
    pub fn latest(&self) -> Option<GpuFrame> {
        let taken = self.ring.take()?;
        Some(GpuFrame {
            ring: Arc::clone(&self.ring),
            refresh: Arc::clone(&self.refresh),
            taken,
        })
    }
}

/// Frame leased from a ring, from [`GpuFrames::latest`]. Dropping it returns its
/// slot.
pub struct GpuFrame {
    ring: Arc<NativeRing>,
    refresh: Refresh,
    taken: Taken<Native>,
}

// Written out since the refresh has no Debug
impl fmt::Debug for GpuFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GpuFrame")
            .field("taken", &self.taken)
            .finish_non_exhaustive()
    }
}

impl GpuFrame {
    /// Name of the slot. A host imports the slot once, when it first sees the
    /// name.
    pub fn slot(&self) -> SlotId {
        self.taken.id
    }

    /// Width of the slot in pixels.
    pub fn width(&self) -> u32 {
        self.taken.shape.width
    }

    /// Height of the slot in pixels.
    pub fn height(&self) -> u32 {
        self.taken.shape.height
    }

    /// Layout of the slot's pixels.
    pub fn format(&self) -> TextureFormat {
        self.taken.shape.format
    }

    /// Part of the slot that holds the page. All of it but for a frame CEF
    /// letterboxes during a resize.
    pub fn valid_rect(&self) -> PixelRect {
        self.taken.valid
    }

    /// Windows NT handle of the slot's shared D3D11 texture, without a keyed
    /// mutex. Open it once per slot with `ID3D12Device::OpenSharedHandle`,
    /// `ID3D11Device1::OpenSharedResource1` or Vulkan's `D3D11_TEXTURE_BIT`.
    /// The ring writes a slot whenever no lease holds it, which makes a read
    /// outside a lease a read of a frame in progress.
    #[cfg(windows)]
    pub fn handle(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        use std::os::windows::io::AsHandle;
        self.taken.native.as_handle()
    }

    /// macOS IOSurface of the slot. Wrap it once per slot in a texture of the
    /// host's Metal device. The ring writes a slot whenever no lease holds it,
    /// which makes a read outside a lease a read of a frame in progress.
    #[cfg(target_os = "macos")]
    pub fn io_surface(&self) -> &objc2_io_surface::IOSurfaceRef {
        self.taken.native.surface()
    }

    /// Linux dmabuf of the slot, one plane. Import it once per slot. The ring
    /// writes a slot whenever no lease holds it, which makes a read outside a
    /// lease a read of a frame in progress.
    #[cfg(target_os = "linux")]
    pub fn dmabuf(&self) -> super::DmabufPlane<'_> {
        use std::os::fd::AsFd;
        let native = &*self.taken.native;
        super::DmabufPlane {
            fd: native.fd.as_fd(),
            stride: native.stride,
            offset: native.offset,
            size: native.size,
        }
    }

    /// Linux DRM format modifier of the slot's dmabuf.
    #[cfg(target_os = "linux")]
    pub fn modifier(&self) -> u64 {
        self.taken.native.modifier
    }
}

impl Drop for GpuFrame {
    fn drop(&mut self) {
        if self.ring.give_back(self.taken.id) {
            (self.refresh)();
        }
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<GpuFrames>();
    assert_send_sync::<GpuFrame>();
};

#[cfg(test)]
mod tests {
    use super::*;

    const SHAPE: Shape = Shape {
        width: 640,
        height: 360,
        format: TextureFormat::Bgra8,
    };

    const LARGER: Shape = Shape {
        width: 800,
        ..SHAPE
    };

    fn rect(width: u32) -> PixelRect {
        PixelRect {
            x: 0,
            y: 0,
            width,
            height: 1,
        }
    }

    /// Writes a frame into whatever `ring` claims, making a slot when asked,
    /// and returns the slot.
    fn write(ring: &Ring<()>, shape: Shape, width: u32) -> Option<SlotId> {
        let id = match ring.claim(shape) {
            Claim::Slot(id) => id,
            Claim::Make => ring.add(shape, ()),
            Claim::Full => return None,
        };
        ring.publish(id, rect(width));
        Some(id)
    }

    #[test]
    fn a_lease_takes_the_newest_frame_once() {
        let ring = Ring::new();
        write(&ring, SHAPE, 1);
        let newest = write(&ring, SHAPE, 2);
        let taken = ring.take().expect("a frame");
        assert_eq!(Some(taken.id), newest);
        assert_eq!(taken.valid, rect(2));
        assert!(ring.take().is_none());
    }

    #[test]
    fn a_new_slot_comes_only_when_none_is_free() {
        let ring = Ring::new();
        let first = write(&ring, SHAPE, 1);
        // The first frame waits; a mailbox writes over the oldest once full
        let second = write(&ring, SHAPE, 2);
        let third = write(&ring, SHAPE, 3);
        assert_ne!(first, second);
        assert_ne!(second, third);
        assert_eq!(write(&ring, SHAPE, 4), first);
        assert_eq!(write(&ring, SHAPE, 5), second);
    }

    #[test]
    fn a_leased_slot_is_never_written() {
        let ring = Ring::new();
        write(&ring, SHAPE, 1);
        let leased = ring.take().expect("a frame");
        for width in 2..20 {
            assert_ne!(write(&ring, SHAPE, width), Some(leased.id));
        }
        ring.give_back(leased.id);
        let written: Vec<_> = (0..3).filter_map(|_| write(&ring, SHAPE, 1)).collect();
        assert!(written.contains(&leased.id));
    }

    #[test]
    fn frames_drop_once_leases_hold_every_slot() {
        let ring = Ring::new();
        let mut leases = Vec::new();
        for width in 0..3 {
            write(&ring, SHAPE, width);
            leases.push(ring.take().expect("a frame"));
        }
        assert_eq!(ring.claim(SHAPE), Claim::Full);
        ring.give_back(leases[0].id);
        assert_eq!(ring.claim(SHAPE), Claim::Slot(leases[0].id));
    }

    #[test]
    fn a_new_shape_clears_every_slot_no_lease_holds() {
        let ring = Ring::new();
        let leased = write(&ring, SHAPE, 1).expect("a slot");
        let held = ring.take().expect("a frame");
        let waiting = write(&ring, SHAPE, 2).expect("a slot");

        let larger = write(&ring, LARGER, 3).expect("a slot");
        assert!(ring.holds(leased));
        assert!(!ring.holds(waiting));
        assert!(ring.holds(larger));

        // Its lease ended, the old shape's slot leaves too
        ring.give_back(held.id);
        assert!(!ring.holds(leased));
    }

    #[test]
    fn a_name_is_never_given_twice() {
        let ring = Ring::new();
        let first = write(&ring, SHAPE, 1).expect("a slot");
        let second = write(&ring, LARGER, 2).expect("a slot");
        let third = write(&ring, SHAPE, 3).expect("a slot");
        assert_ne!(first, second);
        assert_ne!(first, third);
        assert_ne!(second, third);
    }

    #[test]
    fn a_failed_write_frees_its_slot() {
        let ring: Ring<()> = Ring::new();
        let id = ring.add(SHAPE, ());
        ring.abandon(id);
        assert_eq!(ring.claim(SHAPE), Claim::Slot(id));
    }
}

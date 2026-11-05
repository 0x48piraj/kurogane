//! Windows private device of the frame ring.
//!
//! D3D12's copy queue writes the slots, which keeps a frame's copy clear of the
//! 3D work a host fills the GPU with. D3D11 makes the slots, the kind of texture
//! CEF itself hands out and every host's import takes. Both devices live on CEF's
//! UI thread, the one thread that paints.

use std::cell::{Cell, RefCell};
use std::mem::{ManuallyDrop, transmute_copy};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::rc::{Rc, Weak};

use windows::Win32::Foundation::{CompareObjectHandles, HANDLE, HMODULE, LUID, WAIT_FAILED};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_SINGLETHREADED, D3D11_RESOURCE_MISC_SHARED,
    D3D11_RESOURCE_MISC_SHARED_NTHANDLE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Graphics::Direct3D12::{
    CLSID_D3D12DeviceFactory, D3D12_BOX, D3D12_COMMAND_LIST_TYPE_COPY, D3D12_COMMAND_QUEUE_DESC,
    D3D12_DEVICE_FACTORY_FLAG_DISALLOW_STORING_NEW_DEVICE_AS_SINGLETON, D3D12_FENCE_FLAG_NONE,
    D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT, D3D12_HEAP_TYPE_UPLOAD,
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_BUFFER,
    D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COMMON,
    D3D12_RESOURCE_STATE_GENERIC_READ, D3D12_SUBRESOURCE_FOOTPRINT, D3D12_TEXTURE_COPY_LOCATION,
    D3D12_TEXTURE_COPY_LOCATION_0, D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
    D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, D3D12_TEXTURE_DATA_PITCH_ALIGNMENT,
    D3D12_TEXTURE_LAYOUT_ROW_MAJOR, D3D12_TEXTURE_LAYOUT_UNKNOWN, D3D12CreateDevice,
    D3D12GetInterface, ID3D12CommandAllocator, ID3D12CommandList, ID3D12CommandQueue, ID3D12Device,
    ID3D12DeviceFactory, ID3D12Fence, ID3D12GraphicsCommandList, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_TYPELESS, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM,
    DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
    DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE, IDXGIAdapter1, IDXGIFactory4,
    IDXGIFactory6, IDXGIResource1,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{Interface, PCWSTR};

use super::writer::{Copy, FrameDevice, OpensFrames};
use super::{GpuAdapter, GpuChoice, Shape};
use crate::offscreen::frame::{PixelFrame, PixelRect, SharedTextureFrame, TextureFormat};

/// Textures of CEF's a browser keeps open, more than its pool hands out at a
/// steady size.
const KEPT_OPEN: usize = 4;

/// Windows names a GPU by its LUID, high and low.
pub(super) type AdapterId = (i32, u32);

/// Longest wait on the fence in milliseconds before the device's state is read
/// again. A lost device must not hold CEF's UI thread for good.
const WAIT_SLICE: u32 = 100;

thread_local! {
    /// Device this thread's browsers share, alive while one of them is.
    static DEVICE: RefCell<Weak<Device>> = const { RefCell::new(Weak::new()) };
}

/// Returns the adapter of `choice`.
pub(super) fn adapter(choice: GpuChoice) -> Result<GpuAdapter, String> {
    let chosen: IDXGIAdapter1 = match choice {
        GpuChoice::Adapter(adapter) => return Ok(adapter),
        GpuChoice::HighPerformance => {
            // SAFETY: CreateDXGIFactory1 takes no arguments
            let factory: IDXGIFactory6 =
                unsafe { CreateDXGIFactory1() }.map_err(|e| format!("no DXGI factory: {e}"))?;
            // SAFETY: the call takes plain values
            unsafe { factory.EnumAdapterByGpuPreference(0, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE) }
                .map_err(|e| format!("no adapter: {e}"))?
        }
    };
    // SAFETY: GetDesc1 takes no arguments
    let desc = unsafe { chosen.GetDesc1() }.map_err(|e| format!("no adapter description: {e}"))?;
    if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
        return Err("the only adapter is software".to_owned());
    }
    Ok(GpuAdapter::from_luid(
        desc.AdapterLuid.HighPart,
        desc.AdapterLuid.LowPart,
    ))
}

/// Returns the switch that pins Chromium to `adapter`.
pub(super) fn pin_switch(adapter: GpuAdapter) -> Option<(&'static str, String)> {
    let (high, low) = adapter.luid();
    Some(("use-adapter-luid", format!("{high},{low}")))
}

/// Returns the D3D11 format of `format`, typeless for a host to pick a UNORM
/// or sRGB view.
fn typeless(format: TextureFormat) -> DXGI_FORMAT {
    match format {
        TextureFormat::Rgba8 => DXGI_FORMAT_R8G8B8A8_TYPELESS,
        TextureFormat::Bgra8 => DXGI_FORMAT_B8G8R8A8_TYPELESS,
    }
}

/// Returns the plain format of `format`.
fn plain(format: TextureFormat) -> DXGI_FORMAT {
    match format {
        TextureFormat::Rgba8 => DXGI_FORMAT_R8G8B8A8_UNORM,
        TextureFormat::Bgra8 => DXGI_FORMAT_B8G8R8A8_UNORM,
    }
}

/// Returns whether a texture of `format` holds pixels of `shape`'s layout.
fn same_family(format: DXGI_FORMAT, shape: Shape) -> bool {
    let family = match shape.format {
        TextureFormat::Rgba8 => [
            DXGI_FORMAT_R8G8B8A8_UNORM,
            DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
            DXGI_FORMAT_R8G8B8A8_TYPELESS,
        ],
        TextureFormat::Bgra8 => [
            DXGI_FORMAT_B8G8R8A8_UNORM,
            DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
            DXGI_FORMAT_B8G8R8A8_TYPELESS,
        ],
    };
    family.contains(&format)
}

/// Returns a D3D12 device on `adapter` of its own, where Windows offers one.
///
/// D3D12 otherwise gives a process one device per adapter, the host's too. The
/// debug layer a host turns on later removes that device, which a dev build of
/// a wgpu host does.
fn own_device(adapter: &IDXGIAdapter1) -> Result<ID3D12Device, String> {
    let mut device: Option<ID3D12Device> = None;
    let mut factory: Option<ID3D12DeviceFactory> = None;
    // SAFETY: each out-parameter lives through its call
    unsafe {
        let got = D3D12GetInterface(&CLSID_D3D12DeviceFactory, &raw mut factory);
        match factory.filter(|_| got.is_ok()) {
            Some(factory) => {
                factory
                    .SetFlags(D3D12_DEVICE_FACTORY_FLAG_DISALLOW_STORING_NEW_DEVICE_AS_SINGLETON)
                    .map_err(|e| format!("no device factory: {e}"))?;
                factory.CreateDevice(adapter, D3D_FEATURE_LEVEL_11_0, &raw mut device)
            }
            None => D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_11_0, &raw mut device),
        }
    }
    .map_err(|e| format!("no D3D12 device: {e}"))?;
    device.ok_or_else(|| "no D3D12 device".to_owned())
}

/// Returns the bytes a row of `width` pixels takes in staging memory.
fn row_pitch(width: u32) -> u32 {
    (width * 4).next_multiple_of(D3D12_TEXTURE_DATA_PITCH_ALIGNMENT)
}

/// Returns a copy location of `texture`, borrowed for the copy.
fn location(texture: &ID3D12Resource) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: the location never drops the pointer, which the caller's
        // texture outlives
        pResource: unsafe {
            transmute_copy::<ID3D12Resource, ManuallyDrop<Option<ID3D12Resource>>>(texture)
        },
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    }
}

/// Returns the box of `region`.
fn boxed(region: PixelRect) -> D3D12_BOX {
    D3D12_BOX {
        left: region.x,
        top: region.y,
        front: 0,
        right: region.x + region.width,
        bottom: region.y + region.height,
        back: 1,
    }
}

/// Upload memory of a browser's software frames.
pub(in crate::offscreen) struct Staging {
    buffer: ID3D12Resource,
    mapped: *mut u8,
    shape: Shape,
}

/// Private device of this process's browsers. UI thread alone.
pub(in crate::offscreen) struct Device {
    adapter: GpuAdapter,
    /// Makes the slots
    maker: ID3D11Device,
    d3d12: ID3D12Device,
    queue: ID3D12CommandQueue,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    fence: ID3D12Fence,
    done: OwnedHandle,
    fence_value: Cell<u64>,
}

impl Device {
    /// Returns the device on `adapter` this thread's browsers share.
    pub(super) fn shared(adapter: GpuAdapter) -> Result<Rc<Self>, String> {
        DEVICE.with_borrow_mut(|device| {
            if let Some(device) = device.upgrade().filter(|device| !device.is_lost()) {
                return Ok(device);
            }
            let made = Rc::new(Self::open_on(adapter)?);
            *device = Rc::downgrade(&made);
            Ok(made)
        })
    }

    /// Opens the devices on `adapter`.
    fn open_on(gpu: GpuAdapter) -> Result<Self, String> {
        let (high, low) = gpu.luid();
        let luid = LUID {
            HighPart: high,
            LowPart: low,
        };
        // SAFETY: each call below takes plain values or out-parameters that
        // live through it
        unsafe {
            let factory: IDXGIFactory4 =
                CreateDXGIFactory1().map_err(|e| format!("no DXGI factory: {e}"))?;
            let adapter: IDXGIAdapter1 = factory
                .EnumAdapterByLuid(luid)
                .map_err(|e| format!("no adapter {high},{low}: {e}"))?;

            let mut maker = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_SINGLETHREADED,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&raw mut maker),
                None,
                None,
            )
            .map_err(|e| format!("no D3D11 device: {e}"))?;
            let maker: ID3D11Device = maker.ok_or("no D3D11 device")?;

            let d3d12 = own_device(&adapter)?;

            let desc = D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_COPY,
                ..Default::default()
            };
            let queue: ID3D12CommandQueue = d3d12
                .CreateCommandQueue(&raw const desc)
                .map_err(|e| format!("no copy queue: {e}"))?;
            let allocator: ID3D12CommandAllocator = d3d12
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COPY)
                .map_err(|e| format!("no command allocator: {e}"))?;
            let list: ID3D12GraphicsCommandList = d3d12
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COPY, &allocator, None)
                .map_err(|e| format!("no command list: {e}"))?;
            list.Close().map_err(|e| format!("no command list: {e}"))?;
            let fence: ID3D12Fence = d3d12
                .CreateFence(0, D3D12_FENCE_FLAG_NONE)
                .map_err(|e| format!("no fence: {e}"))?;
            let done = CreateEventW(None, false, false, PCWSTR::null())
                .map_err(|e| format!("no event: {e}"))?;
            Ok(Self {
                adapter: gpu,
                maker,
                d3d12,
                queue,
                allocator,
                list,
                fence,
                done: OwnedHandle::from_raw_handle(done.0),
                fence_value: Cell::new(0),
            })
        }
    }

    /// Opens the texture behind `handle` for reading or writing.
    fn open_texture(&self, handle: HANDLE) -> Result<ID3D12Resource, String> {
        let mut texture: Option<ID3D12Resource> = None;
        // SAFETY: handle is a live NT handle of a shared texture
        unsafe { self.d3d12.OpenSharedHandle(handle, &raw mut texture) }
            .map_err(|e| format!("a texture does not open on Kurogane's device: {e}"))?;
        texture.ok_or_else(|| "a texture opened as nothing".to_owned())
    }

    /// Waits for the fence to reach `value`, reading the device's state while
    /// it does not.
    fn wait(&self, value: u64) -> Result<(), String> {
        loop {
            // SAFETY: neither call takes anything that can dangle; done is the
            // event this device owns
            unsafe {
                let reached = self.fence.GetCompletedValue();
                // A removed device reads as every bit set
                if reached != u64::MAX && reached >= value {
                    return Ok(());
                }
                self.d3d12
                    .GetDeviceRemovedReason()
                    .map_err(|e| format!("the private device is lost: {e}"))?;
                if WaitForSingleObject(HANDLE(self.done.as_raw_handle()), WAIT_SLICE) == WAIT_FAILED
                {
                    return Err("the fence's event cannot be waited on".to_owned());
                }
            }
        }
    }

    /// Records one of the frame's copies.
    ///
    /// # Safety
    ///
    /// The list is open and every resource of `copy` lives until the wait ends.
    unsafe fn record(&self, copy: &Copy<'_, Self>) {
        let (target, x, y, source, area) = match copy {
            Copy::Frame { from, to, region } => {
                (location(to), region.x, region.y, location(from), *region)
            }
            Copy::Layer {
                from,
                to,
                width,
                height,
            } => (location(to), 0, 0, location(from), origin(*width, *height)),
            Copy::Popup { from, to, place } => (
                location(to),
                place.x,
                place.y,
                location(from),
                origin(place.width, place.height),
            ),
            Copy::Staged { from, to, region } => {
                (location(to), region.x, region.y, footprint(from), *region)
            }
        };
        let area = boxed(area);
        // SAFETY: the caller's promise; the locations and the box live through
        // the call, borrowed from resources the copy holds
        unsafe {
            self.list.CopyTextureRegion(
                &raw const target,
                x,
                y,
                0,
                &raw const source,
                Some(&raw const area),
            );
        }
    }
}

/// Returns the rectangle of `width` by `height` at the top-left corner.
fn origin(width: u32, height: u32) -> PixelRect {
    PixelRect {
        x: 0,
        y: 0,
        width,
        height,
    }
}

/// Returns a copy location of `staging`, borrowed for the copy.
fn footprint(staging: &Staging) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: the location never drops the pointer, which the caller's
        // staging memory outlives
        pResource: unsafe {
            transmute_copy::<ID3D12Resource, ManuallyDrop<Option<ID3D12Resource>>>(&staging.buffer)
        },
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                Offset: 0,
                Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                    Format: plain(staging.shape.format),
                    Width: staging.shape.width,
                    Height: staging.shape.height,
                    Depth: 1,
                    RowPitch: row_pitch(staging.shape.width),
                },
            },
        },
    }
}

impl FrameDevice for Device {
    type Native = OwnedHandle;
    type Slot = ID3D12Resource;
    type Texture = ID3D12Resource;
    type Kept = Vec<Kept>;
    type Staging = Staging;

    fn make_slot(&self, shape: Shape) -> Result<(OwnedHandle, ID3D12Resource), String> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: shape.width,
            Height: shape.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: typeless(shape.format),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0)
                as u32,
        };
        let mut texture = None;
        // SAFETY: desc and texture live through the call, which reads no data
        unsafe {
            self.maker
                .CreateTexture2D(&raw const desc, None, Some(&raw mut texture))
        }
        .map_err(|e| format!("no slot of {}x{}: {e}", shape.width, shape.height))?;
        let texture = texture.ok_or("no slot")?;
        let resource: IDXGIResource1 = texture.cast().map_err(|e| format!("no slot: {e}"))?;
        let access = DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0;
        // SAFETY: a null name makes the handle unnamed; the texture is shareable
        let handle = unsafe { resource.CreateSharedHandle(None, access, PCWSTR::null()) }
            .map_err(|e| format!("no handle of a slot: {e}"))?;
        let opened = self.open_texture(handle);
        // SAFETY: CreateSharedHandle gave this handle to the caller alone
        let handle = unsafe { OwnedHandle::from_raw_handle(handle.0) };
        Ok((handle, opened?))
    }

    fn make_layer(&self, shape: Shape) -> Result<ID3D12Resource, String> {
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: u64::from(shape.width),
            Height: shape.height,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: plain(shape.format),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_NONE,
        };
        let mut layer: Option<ID3D12Resource> = None;
        // SAFETY: every pointer lives through the call
        unsafe {
            self.d3d12.CreateCommittedResource(
                &raw const heap,
                D3D12_HEAP_FLAG_NONE,
                &raw const desc,
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &raw mut layer,
            )
        }
        .map_err(|e| format!("no popup layer: {e}"))?;
        layer.ok_or_else(|| "no popup layer".to_owned())
    }

    fn make_staging(&self, shape: Shape) -> Result<Staging, String> {
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_UPLOAD,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Alignment: 0,
            Width: u64::from(row_pitch(shape.width)) * u64::from(shape.height),
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_NONE,
        };
        let mut buffer: Option<ID3D12Resource> = None;
        // SAFETY: every pointer lives through the call
        unsafe {
            self.d3d12.CreateCommittedResource(
                &raw const heap,
                D3D12_HEAP_FLAG_NONE,
                &raw const desc,
                D3D12_RESOURCE_STATE_GENERIC_READ,
                None,
                &raw mut buffer,
            )
        }
        .map_err(|e| format!("no staging memory: {e}"))?;
        let buffer = buffer.ok_or("no staging memory")?;
        let mut mapped = std::ptr::null_mut();
        // SAFETY: an upload buffer maps whole and stays mapped for its life
        unsafe { buffer.Map(0, None, Some(&raw mut mapped)) }
            .map_err(|e| format!("no staging map: {e}"))?;
        Ok(Staging {
            buffer,
            mapped: mapped.cast(),
            shape,
        })
    }

    fn stage(&self, staging: &mut Staging, frame: &PixelFrame<'_>, region: PixelRect) {
        let pitch = row_pitch(staging.shape.width) as usize;
        let stride = frame.stride();
        let row_bytes = region.width as usize * 4;
        for row in region.y as usize..(region.y + region.height) as usize {
            let start = row * stride + region.x as usize * 4;
            let from = &frame.bytes()[start..start + row_bytes];
            // SAFETY: the mapping holds pitch bytes for each row of the shape,
            // whose region lies inside it; no copy runs while this writes
            unsafe {
                let to = staging.mapped.add(row * pitch + region.x as usize * 4);
                std::ptr::copy_nonoverlapping(from.as_ptr(), to, row_bytes);
            }
        }
    }

    fn run(&self, copies: &[Copy<'_, Self>]) -> Result<(), String> {
        if copies.is_empty() {
            return Ok(());
        }
        let value = self.fence_value.get() + 1;
        self.fence_value.set(value);
        // SAFETY: the wait below ends the allocator's last use, which leaves no
        // list running at each reset; the copies' resources outlive the wait
        unsafe {
            self.allocator
                .Reset()
                .map_err(|e| format!("no allocator reset: {e}"))?;
            self.list
                .Reset(&self.allocator, None)
                .map_err(|e| format!("no list reset: {e}"))?;
            for copy in copies {
                self.record(copy);
            }
            self.list
                .Close()
                .map_err(|e| format!("no list close: {e}"))?;
            let list: ID3D12CommandList = self.list.cast().map_err(|e| format!("no list: {e}"))?;
            self.queue.ExecuteCommandLists(&[Some(list)]);
            self.queue
                .Signal(&self.fence, value)
                .map_err(|e| format!("no signal: {e}"))?;
            self.fence
                .SetEventOnCompletion(value, HANDLE(self.done.as_raw_handle()))
                .map_err(|e| format!("no fence event: {e}"))?;
        }
        self.wait(value)
    }

    fn is_lost(&self) -> bool {
        // SAFETY: GetDeviceRemovedReason takes no arguments
        unsafe { self.d3d12.GetDeviceRemovedReason() }.is_err()
    }

    fn reopen(&self) -> Result<Rc<Self>, String> {
        Self::shared(self.adapter)
    }
}

/// CEF's texture, kept open between frames.
pub(in crate::offscreen) struct Kept {
    /// Keeps the texture's kernel object from being reused for another
    handle: OwnedHandle,
    resource: ID3D12Resource,
}

impl OpensFrames for Device {
    fn open(
        &self,
        kept: &mut Vec<Kept>,
        frame: &SharedTextureFrame<'_>,
        shape: Shape,
    ) -> Result<ID3D12Resource, String> {
        let handle = frame.handle();
        let raw = HANDLE(handle.as_raw_handle());
        let found = kept.iter().find(|kept| {
            let kept = HANDLE(kept.handle.as_raw_handle());
            // SAFETY: both handles are live; the call reads them alone
            unsafe { CompareObjectHandles(kept, raw) }.as_bool()
        });
        if let Some(found) = found {
            return Ok(found.resource.clone());
        }

        let resource = self.open_texture(raw)?;
        // SAFETY: GetDesc takes no arguments
        let desc = unsafe { resource.GetDesc() };
        if desc.Width != u64::from(shape.width)
            || desc.Height != shape.height
            || !same_family(desc.Format, shape)
        {
            return Err(format!(
                "CEF's texture is {}x{}, its frame {}x{}",
                desc.Width, desc.Height, shape.width, shape.height
            ));
        }
        let handle = handle
            .try_clone_to_owned()
            .map_err(|e| format!("no handle of CEF's texture: {e}"))?;
        if kept.len() == KEPT_OPEN {
            kept.remove(0);
        }
        kept.push(Kept {
            handle,
            resource: resource.clone(),
        });
        Ok(resource)
    }
}

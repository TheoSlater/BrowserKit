use browserkit_types::{LogicalRect, PageId, PageViewBounds};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SurfaceBounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PageViewport {
    pub rect: LogicalRect,
    pub width_px: u32,
    pub height_px: u32,
    pub scale_factor: f64,
}

impl Default for PageViewport {
    fn default() -> Self {
        Self {
            rect: LogicalRect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            width_px: 1,
            height_px: 1,
            scale_factor: 1.0,
        }
    }
}

pub(crate) fn viewport_from_bounds(bounds: &PageViewBounds) -> Result<PageViewport, String> {
    bounds.validate().map_err(str::to_owned)?;
    Ok(PageViewport {
        rect: bounds.rect,
        width_px: physical_dimension(bounds.rect.width, bounds.device_pixel_ratio, "width")?,
        height_px: physical_dimension(bounds.rect.height, bounds.device_pixel_ratio, "height")?,
        // BrowserView reports CSS pixels per device pixel. visualViewport.scale is
        // page zoom/pinch state, not another backing-buffer scale.
        scale_factor: bounds.device_pixel_ratio,
    })
}

fn physical_dimension(value: f64, scale_factor: f64, name: &str) -> Result<u32, String> {
    if value == 0.0 {
        return Ok(0);
    }
    let pixels = (value * scale_factor).round();
    if !pixels.is_finite() || pixels < 1.0 || pixels > u32::MAX as f64 {
        return Err(format!("{name} is outside OSR backing-buffer bounds"));
    }
    Ok(pixels as u32)
}

pub(crate) fn surface_bounds(viewport: &PageViewport) -> Result<SurfaceBounds, String> {
    Ok(SurfaceBounds {
        x: rounded_i32(viewport.rect.x * viewport.scale_factor, "x")?,
        y: rounded_i32(viewport.rect.y * viewport.scale_factor, "y")?,
        width: i32::try_from(viewport.width_px).map_err(|_| "width is outside native bounds")?,
        height: i32::try_from(viewport.height_px).map_err(|_| "height is outside native bounds")?,
    })
}

fn rounded_i32(value: f64, name: &str) -> Result<i32, String> {
    let value = value.round();
    if value.is_finite() && value >= i32::MIN as f64 && value <= i32::MAX as f64 {
        Ok(value as i32)
    } else {
        Err(format!("{name} is outside native bounds"))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DirtyRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrameKind {
    View,
    Popup,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputEvent {
    MouseMove {
        x_px: i32,
        y_px: i32,
        modifiers: u32,
    },
    MouseLeave {
        x_px: i32,
        y_px: i32,
        modifiers: u32,
    },
    MouseButton {
        x_px: i32,
        y_px: i32,
        button: u32,
        mouse_up: bool,
        click_count: i32,
        modifiers: u32,
    },
    Wheel {
        x_px: i32,
        y_px: i32,
        delta_x: i32,
        delta_y: i32,
        modifiers: u32,
    },
    Key {
        key_code: i32,
        native_key_code: i32,
        character: Option<u16>,
        modifiers: u32,
        pressed: bool,
    },
    Focus(bool),
}

// Borrow CEF's callback buffer only for the callback; PageSurface owns one
// reusable presentation buffer per frame kind.
pub(crate) trait PageFrameSource {
    fn receive_frame(
        &mut self,
        kind: FrameKind,
        dirty_rects: &[DirtyRect],
        buffer: &[u8],
        width: u32,
        height: u32,
    ) -> Result<(), String>;
}

#[derive(Clone, Debug)]
pub(crate) struct StoredFrame {
    width: u32,
    height: u32,
    // CEF RenderHandler::on_paint supplies BGRA pixels, tightly packed.
    pixels: Vec<u8>,
    dirty_rects: Vec<DirtyRect>,
}

impl StoredFrame {
    fn new(width: u32, height: u32) -> Result<Self, String> {
        let len = pixel_len(width, height)?;
        Ok(Self {
            width,
            height,
            pixels: vec![0; len],
            dirty_rects: Vec::new(),
        })
    }

    fn replace(
        &mut self,
        buffer: &[u8],
        width: u32,
        height: u32,
        dirty_rects: &[DirtyRect],
    ) -> Result<(), String> {
        let resized = self.width != width || self.height != height;
        if resized {
            self.width = width;
            self.height = height;
            self.pixels.resize(pixel_len(width, height)?, 0);
        }
        let rects = if resized || dirty_rects.is_empty() {
            vec![DirtyRect {
                x: 0,
                y: 0,
                width,
                height,
            }]
        } else {
            dirty_rects.to_vec()
        };
        let source_stride = width as usize * 4;
        for rect in &rects {
            for row in 0..rect.height as usize {
                let source_start = (rect.y as usize + row) * source_stride + rect.x as usize * 4;
                let destination_start = source_start;
                let row_len = rect.width as usize * 4;
                self.pixels[destination_start..destination_start + row_len]
                    .copy_from_slice(&buffer[source_start..source_start + row_len]);
            }
        }
        self.dirty_rects = rects;
        Ok(())
    }
}

fn pixel_len(width: u32, height: u32) -> Result<usize, String> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| "OSR frame size overflow".to_string())
}

fn clip_dirty_rects(rects: &[crate::RawDirtyRect], width: u32, height: u32) -> Vec<DirtyRect> {
    rects
        .iter()
        .filter_map(|rect| {
            let left = (rect.x as i64).clamp(0, width as i64) as u32;
            let top = (rect.y as i64).clamp(0, height as i64) as u32;
            let right = (rect.x as i64 + rect.width as i64).clamp(0, width as i64) as u32;
            let bottom = (rect.y as i64 + rect.height as i64).clamp(0, height as i64) as u32;
            (left < right && top < bottom).then_some(DirtyRect {
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RenderMetrics {
    pub frames_received: u64,
    pub frames_presented: u64,
    pub frames_dropped: u64,
    pub last_frame_size: Option<(u32, u32)>,
    pub paint_time: Duration,
    pub upload_time: Duration,
    pub present_time: Duration,
}

pub(crate) struct PageSurface {
    pub page_id: PageId,
    pub viewport: PageViewport,
    pub visible: bool,
    frame: Option<StoredFrame>,
    popup: Option<(crate::PopupRect, StoredFrame)>,
    pub metrics: RenderMetrics,
}

impl PageSurface {
    pub(crate) fn new(page_id: PageId) -> Self {
        Self {
            page_id,
            viewport: PageViewport::default(),
            visible: false,
            frame: None,
            popup: None,
            metrics: RenderMetrics::default(),
        }
    }

    pub(crate) fn set_viewport(&mut self, viewport: PageViewport) -> bool {
        let changed = self.viewport != viewport;
        self.viewport = viewport;
        changed
    }

    pub(crate) fn set_popup_rect(&mut self, rect: Option<crate::PopupRect>) {
        if let Some(rect) = rect {
            if let Some((old_rect, _)) = self.popup.as_mut() {
                *old_rect = rect;
            } else if let Ok(frame) = StoredFrame::new(1, 1) {
                self.popup = Some((rect, frame));
            }
        } else {
            self.popup = None;
        }
    }

    pub(crate) fn frame(&self) -> Option<&StoredFrame> {
        self.frame.as_ref()
    }

    pub(crate) fn popup_frame(&self) -> Option<(&crate::PopupRect, &StoredFrame)> {
        self.popup.as_ref().map(|(rect, frame)| (rect, frame))
    }

    pub(crate) fn popup_frame_mut(&mut self) -> Option<&mut StoredFrame> {
        self.popup.as_mut().map(|(_, frame)| frame)
    }

    pub(crate) fn page_id(&self) -> PageId {
        self.page_id
    }
}

impl PageFrameSource for PageSurface {
    fn receive_frame(
        &mut self,
        kind: FrameKind,
        dirty_rects: &[DirtyRect],
        buffer: &[u8],
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        pixel_len(width, height)?;
        if buffer.len() < pixel_len(width, height)? {
            return Err("CEF OSR buffer is smaller than declared frame".into());
        }
        match kind {
            FrameKind::View => {
                if let Some(frame) = self.frame.as_mut() {
                    frame.replace(buffer, width, height, dirty_rects)?;
                } else {
                    let mut frame = StoredFrame::new(width, height)?;
                    frame.replace(buffer, width, height, dirty_rects)?;
                    self.frame = Some(frame);
                }
            }
            FrameKind::Popup => {
                let Some(frame) = self.popup_frame_mut() else {
                    return Ok(());
                };
                frame.replace(buffer, width, height, dirty_rects)?;
            }
        }
        self.metrics.frames_received += 1;
        self.metrics.last_frame_size = Some((width, height));
        Ok(())
    }
}

pub(crate) fn clipped_dirty_rects(
    rects: &[crate::RawDirtyRect],
    width: u32,
    height: u32,
) -> Vec<DirtyRect> {
    clip_dirty_rects(rects, width, height)
}

#[cfg(target_os = "linux")]
mod x11 {
    use super::{PageSurface, SurfaceBounds};
    use crate::{enqueue, CommandQueue};
    use std::{
        ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void, CString},
        mem, ptr,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        thread,
        time::Duration,
    };

    type Bool = c_int;
    type Window = c_ulong;
    type Cursor = c_ulong;
    type Time = c_ulong;

    #[repr(C)]
    struct Display {
        _private: [u8; 0],
    }
    #[repr(C)]
    struct Visual {
        _private: [u8; 0],
    }
    #[repr(C)]
    struct XImage {
        _private: [u8; 0],
    }
    #[repr(C)]
    struct GC {
        _private: [u8; 0],
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct XKeyEvent {
        type_: c_int,
        serial: c_ulong,
        send_event: Bool,
        display: *mut Display,
        window: Window,
        root: Window,
        subwindow: Window,
        time: Time,
        x: c_int,
        y: c_int,
        x_root: c_int,
        y_root: c_int,
        state: c_uint,
        keycode: c_uint,
        same_screen: Bool,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct XButtonEvent {
        type_: c_int,
        serial: c_ulong,
        send_event: Bool,
        display: *mut Display,
        window: Window,
        root: Window,
        subwindow: Window,
        time: Time,
        x: c_int,
        y: c_int,
        x_root: c_int,
        y_root: c_int,
        state: c_uint,
        button: c_uint,
        same_screen: Bool,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct XMotionEvent {
        type_: c_int,
        serial: c_ulong,
        send_event: Bool,
        display: *mut Display,
        window: Window,
        root: Window,
        subwindow: Window,
        time: Time,
        x: c_int,
        y: c_int,
        x_root: c_int,
        y_root: c_int,
        state: c_uint,
        is_hint: c_char,
        same_screen: Bool,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct XFocusEvent {
        type_: c_int,
        serial: c_ulong,
        send_event: Bool,
        display: *mut Display,
        window: Window,
        mode: c_int,
        detail: c_int,
    }

    #[repr(C)]
    union XEvent {
        type_: c_int,
        key: XKeyEvent,
        button: XButtonEvent,
        motion: XMotionEvent,
        focus: XFocusEvent,
    }

    #[link(name = "X11")]
    unsafe extern "C" {
        fn XInitThreads() -> c_int;
        fn XOpenDisplay(name: *const c_char) -> *mut Display;
        fn XCloseDisplay(display: *mut Display) -> c_int;
        fn XDefaultScreen(display: *mut Display) -> c_int;
        fn XDefaultVisual(display: *mut Display, screen: c_int) -> *mut Visual;
        fn XDefaultDepth(display: *mut Display, screen: c_int) -> c_int;
        fn XCreateSimpleWindow(
            display: *mut Display,
            parent: Window,
            x: c_int,
            y: c_int,
            width: c_uint,
            height: c_uint,
            border_width: c_uint,
            border: c_ulong,
            background: c_ulong,
        ) -> Window;
        fn XSelectInput(display: *mut Display, window: Window, event_mask: c_long) -> c_int;
        fn XMapRaised(display: *mut Display, window: Window) -> c_int;
        fn XUnmapWindow(display: *mut Display, window: Window) -> c_int;
        fn XMoveResizeWindow(
            display: *mut Display,
            window: Window,
            x: c_int,
            y: c_int,
            width: c_uint,
            height: c_uint,
        ) -> c_int;
        fn XRaiseWindow(display: *mut Display, window: Window) -> c_int;
        fn XDestroyWindow(display: *mut Display, window: Window) -> c_int;
        fn XFlush(display: *mut Display) -> c_int;
        fn XCreateGC(
            display: *mut Display,
            drawable: Window,
            valuemask: c_ulong,
            values: *mut c_void,
        ) -> *mut GC;
        fn XFreeGC(display: *mut Display, gc: *mut GC) -> c_int;
        fn XCreateImage(
            display: *mut Display,
            visual: *mut Visual,
            depth: c_uint,
            format: c_int,
            offset: c_int,
            data: *mut c_char,
            width: c_uint,
            height: c_uint,
            bitmap_pad: c_int,
            bytes_per_line: c_int,
        ) -> *mut XImage;
        fn XPutImage(
            display: *mut Display,
            drawable: Window,
            gc: *mut GC,
            image: *mut XImage,
            src_x: c_int,
            src_y: c_int,
            dest_x: c_int,
            dest_y: c_int,
            width: c_uint,
            height: c_uint,
        ) -> c_int;
        fn XFree(data: *mut c_void) -> c_int;
        fn XClearWindow(display: *mut Display, window: Window) -> c_int;
        fn XDefineCursor(display: *mut Display, window: Window, cursor: Cursor) -> c_int;
        fn XCreateFontCursor(display: *mut Display, shape: c_uint) -> Cursor;
        fn XFreeCursor(display: *mut Display, cursor: Cursor) -> c_int;
        fn XSetInputFocus(
            display: *mut Display,
            focus: Window,
            revert_to: c_int,
            time: Time,
        ) -> c_int;
        fn XGrabPointer(
            display: *mut Display,
            grab_window: Window,
            owner_events: Bool,
            event_mask: c_uint,
            pointer_mode: c_int,
            keyboard_mode: c_int,
            confine_to: Window,
            cursor: Cursor,
            time: Time,
        ) -> c_int;
        fn XUngrabPointer(display: *mut Display, time: Time) -> c_int;
        fn XPending(display: *mut Display) -> c_int;
        fn XNextEvent(display: *mut Display, event: *mut XEvent) -> c_int;
        fn XLookupString(
            event: *mut XKeyEvent,
            buffer: *mut c_char,
            length: c_int,
            keysym: *mut c_ulong,
            status: *mut c_void,
        ) -> c_int;
    }

    const Z_PIXMAP: c_int = 2;
    const EVENT_MASK: c_long =
        (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 21);
    const BUTTON_PRESS: c_int = 4;
    const BUTTON_RELEASE: c_int = 5;
    const MOTION_NOTIFY: c_int = 6;
    const ENTER_NOTIFY: c_int = 7;
    const LEAVE_NOTIFY: c_int = 8;
    const FOCUS_IN: c_int = 9;
    const FOCUS_OUT: c_int = 10;
    const SHIFT_MASK: c_uint = 1;
    const LOCK_MASK: c_uint = 2;
    const CONTROL_MASK: c_uint = 4;
    const MOD1_MASK: c_uint = 8;
    const BUTTON1_MASK: c_uint = 1 << 8;
    const BUTTON2_MASK: c_uint = 1 << 9;
    const BUTTON3_MASK: c_uint = 1 << 10;
    const GRAB_MODE_ASYNC: c_int = 1;
    const REVERT_TO_PARENT: c_int = 2;
    const CURRENT_TIME: Time = 0;

    struct ImageSlot {
        image: *mut XImage,
        data: *mut u8,
        len: usize,
        width: u32,
        height: u32,
    }

    impl ImageSlot {
        fn new(
            display: *mut Display,
            visual: *mut Visual,
            depth: c_int,
            width: u32,
            height: u32,
        ) -> Result<Self, String> {
            let len = (width as usize)
                .checked_mul(height as usize)
                .and_then(|value| value.checked_mul(4))
                .ok_or_else(|| "X11 image size overflow".to_string())?;
            let data = unsafe { libc::malloc(len) }.cast::<u8>();
            if data.is_null() {
                return Err("X11 image allocation failed".into());
            }
            let image = unsafe {
                XCreateImage(
                    display,
                    visual,
                    depth as c_uint,
                    Z_PIXMAP,
                    0,
                    data.cast(),
                    width,
                    height,
                    32,
                    0,
                )
            };
            if image.is_null() {
                unsafe { libc::free(data.cast()) };
                return Err("X11 image creation failed".into());
            }
            Ok(Self {
                image,
                data,
                len,
                width,
                height,
            })
        }

        fn copy(&mut self, pixels: &[u8]) {
            let len = self.len.min(pixels.len());
            unsafe { ptr::copy_nonoverlapping(pixels.as_ptr(), self.data, len) };
        }
    }

    impl Drop for ImageSlot {
        fn drop(&mut self) {
            unsafe {
                XFree(self.image.cast());
                libc::free(self.data.cast());
            }
        }
    }

    pub(crate) struct NativeCompositor {
        display: *mut Display,
        window: Window,
        gc: *mut GC,
        visual: *mut Visual,
        depth: c_int,
        image: Option<ImageSlot>,
        popup_image: Option<ImageSlot>,
        cursor: Cursor,
        visible: bool,
        stop: Arc<AtomicBool>,
        event_thread: Option<thread::JoinHandle<()>>,
    }

    pub(crate) fn initialize_threads() {
        unsafe { XInitThreads() };
    }

    impl NativeCompositor {
        pub(crate) fn new(window_handle: u64, queue: CommandQueue) -> Result<Self, String> {
            if std::env::var("BROWSERKIT_OZONE_PLATFORM").ok().as_deref() != Some("x11") {
                return Err("X11 OSR compositor requires BROWSERKIT_OZONE_PLATFORM=x11".into());
            }
            if window_handle == 0 {
                return Err("CEF did not expose an X11 top-level window handle".into());
            }
            unsafe { XInitThreads() };
            let display = unsafe { XOpenDisplay(ptr::null()) };
            if display.is_null() {
                return Err("X11 display could not be opened".into());
            }
            let screen = unsafe { XDefaultScreen(display) };
            let visual = unsafe { XDefaultVisual(display, screen) };
            let depth = unsafe { XDefaultDepth(display, screen) };
            let window = unsafe {
                XCreateSimpleWindow(display, window_handle as Window, 0, 0, 1, 1, 0, 0, 0)
            };
            let gc = unsafe { XCreateGC(display, window, 0, ptr::null_mut()) };
            if window == 0 || gc.is_null() || visual.is_null() {
                if !gc.is_null() {
                    unsafe { XFreeGC(display, gc) };
                }
                if window != 0 {
                    unsafe { XDestroyWindow(display, window) };
                }
                unsafe { XCloseDisplay(display) };
                return Err("X11 compositor surface could not be created".into());
            }
            unsafe { XSelectInput(display, window, EVENT_MASK) };
            let stop = Arc::new(AtomicBool::new(false));
            let event_thread = spawn_event_thread(window, queue, stop.clone());
            println!("BrowserKit: osr_compositor_created backend=x11");
            Ok(Self {
                display,
                window,
                gc,
                visual,
                depth,
                image: None,
                popup_image: None,
                cursor: 0,
                visible: false,
                stop,
                event_thread: Some(event_thread),
            })
        }

        pub(crate) fn set_bounds(&mut self, bounds: SurfaceBounds) {
            unsafe {
                XMoveResizeWindow(
                    self.display,
                    self.window,
                    bounds.x,
                    bounds.y,
                    bounds.width.max(0) as c_uint,
                    bounds.height.max(0) as c_uint,
                );
                if bounds.width > 0 && bounds.height > 0 {
                    XMapRaised(self.display, self.window);
                    XRaiseWindow(self.display, self.window);
                } else {
                    XUnmapWindow(self.display, self.window);
                }
                XFlush(self.display);
            }
        }

        pub(crate) fn set_visible(&mut self, visible: bool) {
            self.visible = visible;
            unsafe {
                if visible {
                    XMapRaised(self.display, self.window);
                    XRaiseWindow(self.display, self.window);
                } else {
                    XUnmapWindow(self.display, self.window);
                }
                XFlush(self.display);
            }
        }

        pub(crate) fn present(&mut self, surface: &mut PageSurface) {
            if !self.visible || !surface.visible {
                return;
            }
            let started = std::time::Instant::now();
            let Some((frame_width, frame_height)) =
                surface.frame().map(|frame| (frame.width, frame.height))
            else {
                unsafe { XClearWindow(self.display, self.window) };
                return;
            };
            // ponytail: full-surface clear; clip to old popup when flicker matters.
            unsafe { XClearWindow(self.display, self.window) };
            let needs_new_image = self
                .image
                .as_ref()
                .is_none_or(|image| image.width != frame_width || image.height != frame_height);
            if needs_new_image {
                self.image = ImageSlot::new(
                    self.display,
                    self.visual,
                    self.depth,
                    frame_width,
                    frame_height,
                )
                .ok();
            }
            let Some(image) = self.image.as_mut() else {
                surface.metrics.frames_dropped += 1;
                eprintln!(
                    "BrowserKit: osr_frame_dropped page={:?} error=X11 image allocation",
                    surface.page_id()
                );
                return;
            };
            let upload_started = std::time::Instant::now();
            if let Some(frame) = surface.frame() {
                image.copy(&frame.pixels);
            }
            surface.metrics.upload_time += upload_started.elapsed();
            unsafe {
                XPutImage(
                    self.display,
                    self.window,
                    self.gc,
                    image.image,
                    0,
                    0,
                    0,
                    0,
                    frame_width,
                    frame_height,
                );
            }
            if let Some((popup_rect, popup)) = surface.popup_frame() {
                let popup_new = self
                    .popup_image
                    .as_ref()
                    .is_none_or(|image| image.width != popup.width || image.height != popup.height);
                if popup_new {
                    self.popup_image = ImageSlot::new(
                        self.display,
                        self.visual,
                        self.depth,
                        popup.width,
                        popup.height,
                    )
                    .ok();
                }
                if let Some(image) = self.popup_image.as_mut() {
                    image.copy(&popup.pixels);
                    let scale = surface.viewport.scale_factor;
                    unsafe {
                        XPutImage(
                            self.display,
                            self.window,
                            self.gc,
                            image.image,
                            0,
                            0,
                            (popup_rect.x * scale).round() as c_int,
                            (popup_rect.y * scale).round() as c_int,
                            popup.width,
                            popup.height,
                        );
                    }
                } else {
                    surface.metrics.frames_dropped += 1;
                    eprintln!(
                        "BrowserKit: osr_frame_dropped page={:?} error=X11 popup image allocation",
                        surface.page_id()
                    );
                }
            }
            unsafe { XFlush(self.display) };
            surface.metrics.present_time += started.elapsed();
            surface.metrics.frames_presented += 1;
        }

        pub(crate) fn set_cursor(&mut self, cursor_type: u32) {
            let shape = match cursor_type {
                2 => 60,                         // hand2
                3 => 152,                        // xterm
                1 => 34,                         // crosshair
                4 => 150,                        // watch
                6 | 13 | 15 | 18 => 108,         // horizontal resize
                7 | 10 | 14 | 19 => 116,         // vertical resize
                8 | 9 | 11 | 12 | 16 | 17 => 52, // diagonal resize
                _ => 68,                         // left_ptr
            };
            unsafe {
                let cursor = XCreateFontCursor(self.display, shape);
                if cursor != 0 {
                    XDefineCursor(self.display, self.window, cursor);
                    if self.cursor != 0 {
                        XFreeCursor(self.display, self.cursor);
                    }
                    self.cursor = cursor;
                    XFlush(self.display);
                }
            }
        }
    }

    impl Drop for NativeCompositor {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(thread) = self.event_thread.take() {
                let _ = thread.join();
            }
            unsafe {
                if self.cursor != 0 {
                    XFreeCursor(self.display, self.cursor);
                }
                if !self.gc.is_null() {
                    XFreeGC(self.display, self.gc);
                }
                XDestroyWindow(self.display, self.window);
                XFlush(self.display);
                XCloseDisplay(self.display);
            }
        }
    }

    fn spawn_event_thread(
        window: Window,
        queue: CommandQueue,
        stop: Arc<AtomicBool>,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let display_name = std::env::var_os("DISPLAY")
                .and_then(|value| CString::new(value.to_string_lossy().as_bytes()).ok());
            let display = unsafe {
                XOpenDisplay(
                    display_name
                        .as_ref()
                        .map_or(ptr::null(), |value| value.as_ptr()),
                )
            };
            if display.is_null() {
                return;
            }
            let mut captured = false;
            let mut last_click: Option<(c_uint, c_int, c_int, Time)> = None;
            while !stop.load(Ordering::Acquire) {
                while unsafe { XPending(display) } > 0 {
                    let mut event = unsafe { mem::zeroed::<XEvent>() };
                    unsafe { XNextEvent(display, &mut event) };
                    let event_type = unsafe { event.type_ };
                    match event_type {
                        MOTION_NOTIFY => {
                            let event = unsafe { event.motion };
                            let _ = enqueue(
                                &queue,
                                crate::PageCommand::Input {
                                    event: crate::InputEvent::MouseMove {
                                        x_px: event.x,
                                        y_px: event.y,
                                        modifiers: modifiers(event.state),
                                    },
                                },
                            );
                        }
                        ENTER_NOTIFY => {}
                        LEAVE_NOTIFY => {
                            let _ = enqueue(
                                &queue,
                                crate::PageCommand::Input {
                                    event: crate::InputEvent::MouseLeave {
                                        x_px: 0,
                                        y_px: 0,
                                        modifiers: 0,
                                    },
                                },
                            );
                        }
                        BUTTON_PRESS | BUTTON_RELEASE => {
                            let event = unsafe { event.button };
                            if event_type == BUTTON_PRESS && matches!(event.button, 1..=3) {
                                captured = true;
                                unsafe {
                                    XSetInputFocus(display, window, REVERT_TO_PARENT, CURRENT_TIME);
                                    XGrabPointer(
                                        display,
                                        window,
                                        1,
                                        (1 << 2) | (1 << 3) | (1 << 6) | (1 << 5),
                                        GRAB_MODE_ASYNC,
                                        GRAB_MODE_ASYNC,
                                        0,
                                        0,
                                        CURRENT_TIME,
                                    );
                                }
                                let _ = enqueue(
                                    &queue,
                                    crate::PageCommand::Input {
                                        event: crate::InputEvent::Focus(true),
                                    },
                                );
                            }
                            if matches!(event.button, 4..=7) {
                                let (delta_x, delta_y) = match event.button {
                                    4 => (0, 120),
                                    5 => (0, -120),
                                    6 => (120, 0),
                                    _ => (-120, 0),
                                };
                                let _ = enqueue(
                                    &queue,
                                    crate::PageCommand::Input {
                                        event: crate::InputEvent::Wheel {
                                            x_px: event.x,
                                            y_px: event.y,
                                            delta_x,
                                            delta_y,
                                            modifiers: modifiers(event.state),
                                        },
                                    },
                                );
                            } else {
                                let click_count = if event_type == BUTTON_PRESS
                                    && matches!(event.button, 1..=3)
                                {
                                    let count = last_click
                                        .filter(|(button, x, y, time)| {
                                            *button == event.button
                                                && event.time.saturating_sub(*time) <= 500
                                                && (event.x - *x).abs() <= 4
                                                && (event.y - *y).abs() <= 4
                                        })
                                        .map_or(1, |_| 2);
                                    last_click = Some((event.button, event.x, event.y, event.time));
                                    count
                                } else {
                                    1
                                };
                                let _ = enqueue(
                                    &queue,
                                    crate::PageCommand::Input {
                                        event: crate::InputEvent::MouseButton {
                                            x_px: event.x,
                                            y_px: event.y,
                                            button: event.button,
                                            mouse_up: event_type == BUTTON_RELEASE,
                                            click_count,
                                            modifiers: modifiers(event.state),
                                        },
                                    },
                                );
                            }
                            if event_type == BUTTON_RELEASE
                                && captured
                                && matches!(event.button, 1..=3)
                            {
                                captured = false;
                                unsafe { XUngrabPointer(display, CURRENT_TIME) };
                            }
                        }
                        2 | 3 => {
                            let mut event = unsafe { event.key };
                            let mut text = [0 as c_char; 8];
                            let mut keysym = 0;
                            let text_len = unsafe {
                                XLookupString(
                                    &mut event,
                                    text.as_mut_ptr(),
                                    text.len() as c_int,
                                    &mut keysym,
                                    ptr::null_mut(),
                                )
                            };
                            let character = (text_len == 1)
                                .then_some(text[0] as u8 as u16)
                                .filter(|character| *character >= 0x20);
                            let _ = enqueue(
                                &queue,
                                crate::PageCommand::Input {
                                    event: crate::InputEvent::Key {
                                        key_code: key_code(keysym as u32),
                                        native_key_code: event.keycode as i32,
                                        character,
                                        modifiers: modifiers(event.state),
                                        pressed: event_type == 2,
                                    },
                                },
                            );
                        }
                        FOCUS_IN => {
                            let _ = enqueue(
                                &queue,
                                crate::PageCommand::Input {
                                    event: crate::InputEvent::Focus(true),
                                },
                            );
                        }
                        FOCUS_OUT => {
                            let _ = enqueue(
                                &queue,
                                crate::PageCommand::Input {
                                    event: crate::InputEvent::Focus(false),
                                },
                            );
                        }
                        _ => {}
                    }
                }
                thread::sleep(Duration::from_millis(4));
            }
            unsafe { XCloseDisplay(display) };
        })
    }

    fn modifiers(state: c_uint) -> u32 {
        let mut value = 0;
        if state & SHIFT_MASK != 0 {
            value |= 2;
        }
        if state & CONTROL_MASK != 0 {
            value |= 4;
        }
        if state & MOD1_MASK != 0 {
            value |= 8;
        }
        if state & LOCK_MASK != 0 {
            value |= 1;
        }
        if state & BUTTON1_MASK != 0 {
            value |= 16;
        }
        if state & BUTTON2_MASK != 0 {
            value |= 32;
        }
        if state & BUTTON3_MASK != 0 {
            value |= 64;
        }
        value
    }

    fn key_code(keysym: u32) -> i32 {
        match keysym {
            0xff08 => 0x08,
            0xff09 => 0x09,
            0xff0d => 0x0d,
            0xff1b => 0x1b,
            0xffff => 0x2e,
            0xff50 => 0x24,
            0xff51 => 0x25,
            0xff52 => 0x26,
            0xff53 => 0x27,
            0xff54 => 0x28,
            0xff55 => 0x21,
            0xff56 => 0x22,
            0xff57 => 0x23,
            0xffbe..=0xffc9 => 0x70 + (keysym - 0xffbe) as i32,
            value if value <= 0x7f => value as i32,
            value => value as i32,
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use x11::initialize_threads;
#[cfg(target_os = "linux")]
pub(crate) use x11::NativeCompositor;

#[cfg(not(target_os = "linux"))]
pub(crate) struct NativeCompositor;

#[cfg(not(target_os = "linux"))]
pub(crate) fn initialize_threads() {}

#[cfg(not(target_os = "linux"))]
impl NativeCompositor {
    pub(crate) fn new(_window_handle: u64, _queue: crate::CommandQueue) -> Result<Self, String> {
        Err("OSR compositor has no platform backend on this target".into())
    }
    pub(crate) fn set_bounds(&mut self, _bounds: SurfaceBounds) {}
    pub(crate) fn set_visible(&mut self, _visible: bool) {}
    pub(crate) fn present(&mut self, _surface: &mut PageSurface) {}
    pub(crate) fn set_cursor(&mut self, _cursor_type: u32) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use browserkit_types::{CoordinateSpace, LogicalRect, PageViewBounds};

    fn bounds(rect: LogicalRect, dpr: f64) -> PageViewBounds {
        PageViewBounds {
            rect,
            coordinate_space: CoordinateSpace::FrontendLogical,
            device_pixel_ratio: dpr,
            visual_viewport_scale: 1.0,
        }
    }

    #[test]
    fn converts_logical_viewport_to_physical_size() {
        let viewport = viewport_from_bounds(&bounds(
            LogicalRect {
                x: 238.0,
                y: 93.59,
                width: 924.0,
                height: 628.41,
            },
            1.25,
        ))
        .unwrap();
        assert_eq!((viewport.width_px, viewport.height_px), (1155, 786));
        assert_eq!(surface_bounds(&viewport).unwrap().x, 298);
    }

    #[test]
    fn zero_size_viewport_allocates_no_buffer() {
        let viewport = viewport_from_bounds(&bounds(
            LogicalRect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            },
            2.0,
        ))
        .unwrap();
        assert_eq!((viewport.width_px, viewport.height_px), (0, 0));
    }

    #[test]
    fn clips_dirty_rects_to_frame() {
        let rects = clipped_dirty_rects(
            &[
                crate::RawDirtyRect {
                    x: -2,
                    y: 1,
                    width: 5,
                    height: 6,
                },
                crate::RawDirtyRect {
                    x: 20,
                    y: 20,
                    width: 2,
                    height: 2,
                },
            ],
            10,
            10,
        );
        assert_eq!(
            rects,
            [DirtyRect {
                x: 0,
                y: 1,
                width: 3,
                height: 6,
            }]
        );
    }

    #[test]
    fn validates_frame_dimensions_and_reuses_storage() {
        let mut surface = PageSurface::new(PageId::from_raw(1));
        let buffer = vec![7; 4 * 2 * 4];
        surface
            .receive_frame(FrameKind::View, &[], &buffer, 4, 2)
            .unwrap();
        let pointer = surface.frame().unwrap().pixels.as_ptr();
        surface
            .receive_frame(FrameKind::View, &[], &buffer, 4, 2)
            .unwrap();
        assert_eq!(surface.frame().unwrap().pixels.as_ptr(), pointer);
        assert!(surface
            .receive_frame(FrameKind::View, &[], &[0; 3], 4, 2)
            .is_err());
    }
}

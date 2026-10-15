//! The window and event loop: winit for the window, egui-winit for turning
//! window events into egui input, egui-wgpu for drawing. Key presses are
//! offered to the app first, so they can go to the remote computer without
//! egui ever seeing them.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use egui::ViewportId;
use egui_wgpu::winit::Painter;
use egui_wgpu::{RendererOptions, SurfaceConfig, WgpuConfiguration, WgpuSetup};
use egui_winit::winit;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Fullscreen, Window, WindowId};

use crate::app::{App, WindowRequest};
use crate::session::Wake;

/// Messages to the event loop from other threads.
#[derive(Debug)]
pub enum UserEvent {
    /// A session has news (an event or new picture data).
    Wake,
    /// egui asked for a repaint at this time.
    RepaintAt(Instant),
}

pub fn run(connect: Option<String>) -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().map_err(|e| e.to_string())?;
    let proxy = event_loop.create_proxy();
    let mut runner = Runner { proxy, connect, state: None, wake_pending: Arc::new(AtomicBool::new(false)), next_repaint: None, error: None };
    event_loop.run_app(&mut runner).map_err(|e| e.to_string())?;
    if let Some(err) = runner.error {
        return Err(err);
    }
    if let Some(state) = runner.state.take() {
        state.app.shutdown();
    }
    Ok(())
}

struct Runner {
    proxy: EventLoopProxy<UserEvent>,
    connect: Option<String>,
    state: Option<State>,
    /// A wake-up is already on its way; don't flood the event loop.
    wake_pending: Arc<AtomicBool>,
    next_repaint: Option<Instant>,
    error: Option<String>,
}

struct State {
    window: Arc<Window>,
    ctx: egui::Context,
    egui_state: egui_winit::State,
    painter: Painter,
    app: App,
    /// The window is being resized right now (for smooth live resizing on macOS).
    resizing: bool,
}

impl Runner {
    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let attrs = Window::default_attributes()
            .with_title("Mini Remote")
            .with_inner_size(LogicalSize::new(1180.0, 760.0))
            .with_min_inner_size(LogicalSize::new(640.0, 440.0));
        let attrs = crate::platform::window_attributes(attrs);
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);

        let ctx = egui::Context::default();
        let mut config = WgpuConfiguration { surface: SurfaceConfig::LOW_LATENCY, ..Default::default() };
        if let WgpuSetup::CreateNew(create_new) = &mut config.wgpu_setup {
            create_new.display_handle = Some(Box::new(event_loop.owned_display_handle()));
        }
        let mut painter = pollster::block_on(Painter::new(ctx.clone(), config, false, RendererOptions::default()));
        pollster::block_on(painter.set_window(ViewportId::ROOT, Some(window.clone())))
            .map_err(|e| format!("the graphics card could not be used: {e}"))?;
        let gpu = painter.render_state().ok_or("the graphics card could not be used")?;

        let egui_state = egui_winit::State::new(
            ctx.clone(),
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            window.theme(),
            painter.max_texture_side(),
        );

        // Repaints asked for by egui (animations, timers, other threads).
        let proxy = self.proxy.clone();
        ctx.set_request_repaint_callback(move |info| {
            let _ = proxy.send_event(UserEvent::RepaintAt(Instant::now() + info.delay));
        });

        // Sessions call this from network threads when something changed.
        let proxy = self.proxy.clone();
        let pending = self.wake_pending.clone();
        let wake: Wake = Arc::new(move || {
            if !pending.swap(true, Ordering::AcqRel) {
                let _ = proxy.send_event(UserEvent::Wake);
            }
        });

        let app = App::new(ctx.clone(), gpu, wake, self.connect.take());
        window.request_redraw();
        self.state = Some(State { window, ctx, egui_state, painter, app, resizing: false });
        Ok(())
    }
}

impl ApplicationHandler<UserEvent> for Runner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_none()
            && let Err(err) = self.init(event_loop)
        {
            self.error = Some(err);
            event_loop.exit();
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        let Some(state) = &self.state else { return };
        match event {
            UserEvent::Wake => {
                self.wake_pending.store(false, Ordering::Release);
                state.window.request_redraw();
            }
            UserEvent::RepaintAt(at) => {
                if at <= Instant::now() + Duration::from_millis(1) {
                    state.window.request_redraw();
                } else {
                    self.next_repaint = Some(self.next_repaint.map_or(at, |t| t.min(at)));
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };

        // macOS live resize: tell the painter when a resize stream ends.
        let keeps_resizing = matches!(event, WindowEvent::Resized(_))
            || (cfg!(target_os = "macos") && matches!(event, WindowEvent::Moved(_)));
        if state.resizing && !keeps_resizing {
            state.resizing = false;
            state.painter.on_window_resize_state_change(ViewportId::ROOT, false);
        }

        match &event {
            WindowEvent::CloseRequested => {
                state.app.on_exit();
                event_loop.exit();
                return;
            }
            WindowEvent::RedrawRequested => {
                state.paint(event_loop, &mut self.next_repaint);
                return;
            }
            WindowEvent::Resized(size) => {
                if let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
                    if !state.resizing {
                        state.resizing = true;
                        state.painter.on_window_resize_state_change(ViewportId::ROOT, true);
                    }
                    state.painter.on_window_resized(ViewportId::ROOT, w, h);
                }
                state.app.sync_fullscreen(state.window.fullscreen().is_some());
            }
            WindowEvent::Focused(focused) => {
                // winit can report focus wrongly on macOS; ask the window instead.
                let focused = if cfg!(target_os = "macos") { state.window.has_focus() } else { *focused };
                state.app.on_focus(focused);
            }
            WindowEvent::ModifiersChanged(mods) => state.app.on_modifiers(mods.state()),
            WindowEvent::KeyboardInput { event: key, is_synthetic, .. } => {
                if state.app.on_key(key, *is_synthetic) {
                    // The key went to the remote computer (or was an app shortcut).
                    return;
                }
            }
            _ => {}
        }

        let response = state.egui_state.on_window_event(&state.window, &event);
        if response.repaint || matches!(event, WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }) {
            if matches!(event, WindowEvent::Resized(_)) {
                // Paint right away so resizing looks smooth.
                state.paint(event_loop, &mut self.next_repaint);
            } else {
                state.window.request_redraw();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = &self.state else {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        };
        match self.next_repaint {
            Some(at) if at <= Instant::now() => {
                self.next_repaint = None;
                state.window.request_redraw();
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &mut self.state {
            state.painter.destroy();
        }
    }
}

impl State {
    fn paint(&mut self, event_loop: &ActiveEventLoop, next_repaint: &mut Option<Instant>) {
        let raw_input = self.egui_state.take_egui_input(&self.window);
        let app = &mut self.app;
        let full_output = self.ctx.run_ui(raw_input, |ui| crate::ui::show(app, ui));

        self.egui_state.handle_platform_output_with_event_loop(&self.window, event_loop, full_output.platform_output);

        let ppp = full_output.pixels_per_point;
        let clipped = self.ctx.tessellate(full_output.shapes, ppp);
        let mut textures_delta = full_output.textures_delta;
        let clear = self.ctx.global_style().visuals.panel_fill.to_normalized_gamma_f32();
        self.painter.paint_and_update_textures(
            ViewportId::ROOT,
            ppp,
            clear,
            &clipped,
            &mut textures_delta,
            Vec::new(),
            &self.window,
        );

        if let Some(viewport) = full_output.viewport_output.get(&ViewportId::ROOT) {
            let delay = viewport.repaint_delay;
            if delay.is_zero() {
                self.window.request_redraw();
            } else if delay < Duration::from_secs(3600) {
                let at = Instant::now() + delay;
                *next_repaint = Some(next_repaint.map_or(at, |t| t.min(at)));
            }
        }

        for request in self.app.take_window_requests() {
            match request {
                WindowRequest::Title(title) => self.window.set_title(&title),
                WindowRequest::Fullscreen(on) => {
                    self.window.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
                }
            }
        }
    }
}

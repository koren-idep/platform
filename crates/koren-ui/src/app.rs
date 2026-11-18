//! Application window lifecycle and demand-driven `winit` event loop.

use crate::draw::{DrawList, Point, Size};
use crate::input::{FocusId, InputState};
use crate::layout::UiContext;
use crate::renderer::{TextEngine, WgpuRenderer};
use crate::theme::ThemeTokens;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

/// Actions an application can request on its native window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowCommand {
    /// Minimize the window.
    Minimize,
    /// Toggle maximize/restore.
    ToggleMaximize,
    /// Begin OS window drag.
    DragWindow,
    /// Request application exit.
    Close,
    /// Force a redraw on the next frame.
    Redraw,
}

/// Application state trait implemented by Koren Platform desktop apps.
pub trait NativeApp {
    /// Returns the window title.
    fn title(&self) -> &str;

    /// Returns the active theme tokens.
    fn theme(&self) -> &ThemeTokens;

    /// Renders the UI for `viewport` (logical pixels) into `ctx`.
    fn view(&mut self, ctx: &mut UiContext<'_>, viewport: Size);

    /// Handles a mouse click on `target` at logical `pos`. Returns optional window command.
    fn on_click(
        &mut self,
        _target: Option<FocusId>,
        _pos: Point,
        _button: MouseButton,
        _input: &mut InputState,
    ) -> Option<WindowCommand> {
        None
    }

    /// Handles a double click on `target`.
    fn on_double_click(&mut self, _target: Option<FocusId>, _pos: Point) {}

    /// Handles mouse wheel scroll at `pos`.
    fn on_scroll(&mut self, _pos: Point, _dx: f32, _dy: f32) {}

    /// Handles a key press. Returns `true` if handled (triggers redraw).
    fn on_key(
        &mut self,
        _key: &Key,
        _text: Option<&str>,
        _mods: ModifiersState,
        _input: &mut InputState,
    ) -> bool {
        false
    }

    /// Polls any background platform events (e.g. LSP, tasks, workspace updates).
    /// Returns `true` if state changed and a redraw is needed.
    fn poll_platform(&mut self) -> bool {
        false
    }

    /// Optional callback invoked right after the first frame is presented (useful for smoke/benchmarks).
    fn on_first_frame(&mut self) -> Option<WindowCommand> {
        None
    }
}

struct Runner<A: NativeApp> {
    app: A,
    renderer: Option<WgpuRenderer>,
    window: Option<Arc<Window>>,
    text_engine: TextEngine,
    draw_list: DrawList,
    input: InputState,
    user_zoom: f32,
    logical_size: Size,
    first_frame_done: bool,
    decorated: bool,
}

impl<A: NativeApp> Runner<A> {
    fn new(app: A, decorated: bool) -> Self {
        Self {
            app,
            window: None,
            renderer: None,
            text_engine: TextEngine::new(),
            draw_list: DrawList::new(),
            input: InputState::new(),
            user_zoom: 1.0,
            logical_size: Size::new(1280.0, 800.0),
            first_frame_done: false,
            decorated,
        }
    }

    fn effective_scale(&self) -> f32 {
        let dpr = self
            .window
            .as_ref()
            .map_or(1.0, |w| w.scale_factor() as f32);
        (dpr * self.user_zoom).clamp(0.5, 3.0)
    }

    fn effective_logical_viewport(&self) -> Size {
        let Some(win) = &self.window else {
            return self.logical_size;
        };
        let phys = win.inner_size();
        let scale = self.effective_scale();
        Size::new(phys.width as f32 / scale, phys.height as f32 / scale)
    }

    fn exec_window_cmd(&mut self, event_loop: &ActiveEventLoop, cmd: WindowCommand) {
        match cmd {
            WindowCommand::Minimize => {
                if let Some(w) = &self.window {
                    w.set_minimized(true);
                }
            }
            WindowCommand::ToggleMaximize => {
                if let Some(w) = &self.window {
                    w.set_maximized(!w.is_maximized());
                }
            }
            WindowCommand::DragWindow => {
                if let Some(w) = &self.window {
                    let _ = w.drag_window();
                }
            }
            WindowCommand::Close => {
                self.renderer = None;
                self.window = None;
                event_loop.exit();
            }
            WindowCommand::Redraw => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
        }
    }

    fn request_redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

impl<A: NativeApp> ApplicationHandler for Runner<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = WindowAttributes::default()
            .with_title(self.app.title())
            .with_decorations(self.decorated)
            .with_inner_size(LogicalSize::new(1280.0_f64, 800.0_f64))
            .with_min_inner_size(LogicalSize::new(680.0_f64, 440.0_f64));

        let Ok(win) = event_loop.create_window(attrs) else {
            event_loop.exit();
            return;
        };
        win.set_ime_allowed(true);
        let win = Arc::new(win);
        match WgpuRenderer::new(Arc::clone(&win)) {
            Ok(renderer) => {
                self.renderer = Some(renderer);
                self.window = Some(win);
                self.request_redraw();
            }
            Err(err) => {
                eprintln!("koren-ui renderer initialization failed: {err}");
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(new_size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(new_size.width, new_size.height);
                }
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.input.modifiers = mods.state();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.effective_scale();
                self.input.pointer =
                    Point::new(position.x as f32 / scale, position.y as f32 / scale);
                if self.input.update_hover() {
                    self.request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => {
                let target = self.input.hit_test(self.input.pointer).map(|r| r.id);
                if let Some(region) = self.input.hit_test(self.input.pointer)
                    && region.focusable
                {
                    self.input.focused = Some(region.id);
                }
                if let Some(cmd) =
                    self.app
                        .on_click(target, self.input.pointer, button, &mut self.input)
                {
                    self.exec_window_cmd(event_loop, cmd);
                }
                self.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * 36.0, y * 36.0),
                    MouseScrollDelta::PixelDelta(p) => {
                        let scale = self.effective_scale();
                        (p.x as f32 / scale, p.y as f32 / scale)
                    }
                };
                self.app.on_scroll(self.input.pointer, dx, dy);
                self.request_redraw();
            }
            WindowEvent::Ime(Ime::Preedit(text, _)) => {
                self.input.ime_preedit = if text.is_empty() { None } else { Some(text) };
                self.request_redraw();
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                self.input.ime_preedit = None;
                let key = Key::Character(text.clone().into());
                if self
                    .app
                    .on_key(&key, Some(&text), self.input.modifiers, &mut self.input)
                {
                    self.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let mods = self.input.modifiers;
                // Built-in UI scaling shortcuts: Ctrl++, Ctrl+-, Ctrl+0
                if mods.control_key() {
                    if let Key::Character(ref ch) = event.logical_key {
                        if ch == "+" || ch == "=" {
                            self.user_zoom = (self.user_zoom + 0.1).min(2.0);
                            self.request_redraw();
                            return;
                        } else if ch == "-" {
                            self.user_zoom = (self.user_zoom - 0.1).max(0.7);
                            self.request_redraw();
                            return;
                        } else if ch == "0" {
                            self.user_zoom = 1.0;
                            self.request_redraw();
                            return;
                        }
                    }
                }
                if matches!(event.logical_key, Key::Named(NamedKey::Tab)) && !mods.control_key() {
                    self.input.cycle_focus(!mods.shift_key());
                    self.request_redraw();
                    return;
                }
                let text = event.text.as_deref();
                if self
                    .app
                    .on_key(&event.logical_key, text, mods, &mut self.input)
                {
                    self.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                let scale = self.effective_scale();
                let viewport = self.effective_logical_viewport();
                self.draw_list.clear();
                self.input.begin_frame();

                let theme = *self.app.theme();
                let mut ctx = UiContext {
                    theme: &theme,
                    draw: &mut self.draw_list,
                    input: &mut self.input,
                    text: &mut self.text_engine,
                    scale,
                };
                self.app.view(&mut ctx, viewport);

                if let Some(renderer) = &mut self.renderer {
                    let _ = renderer.render(
                        theme.canvas_bg,
                        &self.draw_list,
                        &mut self.text_engine,
                        scale,
                    );
                }
                if !self.first_frame_done {
                    self.first_frame_done = true;
                    if let Some(cmd) = self.app.on_first_frame() {
                        self.exec_window_cmd(event_loop, cmd);
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if self.app.poll_platform() {
            self.request_redraw();
        }
    }
}

/// Runs a [`NativeApp`] using a demand-driven `winit` event loop.
pub fn run_app<A: NativeApp>(app: A, decorated: bool) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("failed to create event loop: {e}"))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut runner = Runner::new(app, decorated);
    event_loop
        .run_app(&mut runner)
        .map_err(|e| format!("event loop error: {e}"))
}

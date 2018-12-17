use crate::input::{ContextMenu, ModalMenuState};
use crate::{Canvas, Event, GfxCtx, ModalMenu, TopMenu, UserInput};
use glutin_window::GlutinWindow;
use opengl_graphics::{GlGraphics, OpenGL};
use piston::event_loop::{EventLoop, EventSettings, Events};
use piston::window::{Window, WindowSettings};
use std::{panic, process};

pub trait GUI<T> {
    // Called once
    fn top_menu(_canvas: &Canvas) -> Option<TopMenu> {
        None
    }
    fn modal_menus() -> Vec<ModalMenu> {
        Vec::new()
    }
    fn event(&mut self, input: &mut UserInput) -> (EventLoopMode, T);
    fn get_mut_canvas(&mut self) -> &mut Canvas;
    fn draw(&self, g: &mut GfxCtx, data: &T);
    // Will be called if event or draw panics.
    fn dump_before_abort(&self) {}
    // Only before a normal exit, like window close
    fn before_quit(&self) {}
}

#[derive(Clone, Copy, PartialEq)]
pub enum EventLoopMode {
    Animation,
    InputOnly,
}

pub fn run<T, G: GUI<T>>(mut gui: G, window_title: &str, initial_width: u32, initial_height: u32) {
    let opengl = OpenGL::V3_2;
    let settings = WindowSettings::new(window_title, [initial_width, initial_height])
        .opengl(opengl)
        .exit_on_esc(false)
        // TODO it'd be cool to dynamically tweak antialiasing settings as we zoom in
        .samples(2)
        .srgb(false);
    let mut window: GlutinWindow = settings.build().expect("Could not create window");
    let mut events = Events::new(EventSettings::new().lazy(true));
    let mut gl = GlGraphics::new(opengl);

    // TODO Probably time to bundle this state up. :)
    let mut last_event_mode = EventLoopMode::InputOnly;
    let mut context_menu = ContextMenu::Inactive;
    let mut top_menu = G::top_menu(gui.get_mut_canvas());
    let mut modal_state = ModalMenuState::new(G::modal_menus());
    let mut last_data: Option<T> = None;

    while let Some(ev) = events.next(&mut window) {
        use piston::input::{CloseEvent, RenderEvent};
        if let Some(args) = ev.render_args() {
            // If the very first event is render, then just wait.
            if let Some(ref data) = last_data {
                gl.draw(args.viewport(), |c, g| {
                    let mut g = GfxCtx::new(g, c);
                    gui.get_mut_canvas()
                        .start_drawing(&mut g, window.draw_size());

                    if let Err(err) =
                        panic::catch_unwind(panic::AssertUnwindSafe(|| gui.draw(&mut g, data)))
                    {
                        gui.dump_before_abort();
                        panic::resume_unwind(err);
                    }

                    // Always draw the menus last.
                    if let Some(ref menu) = top_menu {
                        menu.draw(&mut g, gui.get_mut_canvas());
                    }
                    if let Some((_, ref menu)) = modal_state.active {
                        menu.draw(&mut g, gui.get_mut_canvas());
                    }
                    if let ContextMenu::Displaying(ref menu) = context_menu {
                        menu.draw(&mut g, gui.get_mut_canvas());
                    }
                });
            }
        } else if ev.close_args().is_some() {
            gui.before_quit();
            process::exit(0);
        } else {
            // Skip some events.
            use piston::input::{
                AfterRenderEvent, CursorEvent, FocusEvent, IdleEvent, MouseRelativeEvent,
                ResizeEvent, TextEvent,
            };
            if ev.after_render_args().is_some()
                || ev.cursor_args().is_some()
                || ev.focus_args().is_some()
                || ev.idle_args().is_some()
                || ev.mouse_relative_args().is_some()
                || ev.resize_args().is_some()
                || ev.text_args().is_some()
            {
                continue;
            }

            // It's impossible / very unlikey we'll grab the cursor in map space before the very first
            // start_drawing call.
            let mut input = UserInput::new(
                Event::from_piston_event(ev),
                context_menu,
                top_menu,
                modal_state,
                gui.get_mut_canvas(),
            );
            let (new_event_mode, data) =
                match panic::catch_unwind(panic::AssertUnwindSafe(|| gui.event(&mut input))) {
                    Ok(pair) => pair,
                    Err(err) => {
                        gui.dump_before_abort();
                        panic::resume_unwind(err);
                    }
                };
            last_data = Some(data);
            context_menu = input.context_menu.maybe_build(gui.get_mut_canvas());
            top_menu = input.top_menu;
            modal_state = input.modal_state;
            if let Some(action) = input.chosen_action {
                panic!(
                    "\"{}\" chosen from the top or modal menu, but nothing consumed it",
                    action
                );
            }

            // Don't constantly reset the events struct -- only when laziness changes.
            if new_event_mode != last_event_mode {
                events.set_lazy(new_event_mode == EventLoopMode::InputOnly);
                last_event_mode = new_event_mode;
            }
        }
    }
}

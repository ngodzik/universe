//! The browser's window held over universe's, so that the two read as one
//! application: on X11, without the window manager's frame, kept above
//! universe's window and out of the task bar and the pager. Elsewhere the
//! window is only placed, which the DevTools do.

/// The browser's window, held over universe's where the system lets it
/// be.
pub(crate) struct Held {
    #[cfg(target_os = "linux")]
    x11: Option<x11::Held>,
}

impl Held {
    /// The window of the browser's process `pid`, to hold over universe's
    /// window `parent`; found once it shows.
    #[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
    pub(crate) fn new(pid: u32, parent: Option<u64>) -> Self {
        Self {
            #[cfg(target_os = "linux")]
            x11: x11::Held::connect(pid, parent),
        }
    }

    /// No window to hold: a headless browser's.
    pub(crate) fn none() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            x11: None,
        }
    }

    /// Holds the window: called each time it is placed, since the browser
    /// may set its own hints again meanwhile. Whether it is held.
    pub(crate) fn hold(&mut self) -> bool {
        #[cfg(target_os = "linux")]
        if let Some(x11) = &mut self.x11 {
            return x11.hold();
        }
        false
    }

    /// Whether the window can be held here: then it has no frame, and its
    /// page is the whole of it.
    pub(crate) fn can_hold(&self) -> bool {
        #[cfg(target_os = "linux")]
        return self.x11.is_some();
        #[cfg(not(target_os = "linux"))]
        false
    }
}

#[cfg(target_os = "linux")]
mod x11 {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask, MapState, PropMode, Window,
    };
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    /// `_NET_WM_STATE`'s order to add a state.
    const ADD: u32 = 1;

    pub(super) struct Held {
        connection: RustConnection,
        root: Window,
        pid: u32,
        parent: Option<Window>,
        window: Option<Window>,
    }

    impl Held {
        /// A connection to the display universe runs on, if it is X11's.
        pub(super) fn connect(pid: u32, parent: Option<u64>) -> Option<Self> {
            std::env::var_os("DISPLAY")?;
            let (connection, screen) = x11rb::connect(None).ok()?;
            let root = connection.setup().roots.get(screen)?.root;
            Some(Self {
                connection,
                root,
                pid,
                parent: parent.and_then(|p| Window::try_from(p).ok()),
                window: None,
            })
        }

        pub(super) fn hold(&mut self) -> bool {
            if self.window.is_none() {
                self.window = self.find();
            }
            let Some(window) = self.window else {
                return false;
            };
            // Errors are the display's: a window gone meanwhile is found
            // again next time.
            if self.apply(window).is_none() {
                self.window = None;
            }
            self.window.is_some()
        }

        fn apply(&self, window: Window) -> Option<()> {
            let c = &self.connection;
            // No frame of the window manager's, as Motif's hints say to
            // most of them: flags for decorations, and none.
            let motif = self.atom("_MOTIF_WM_HINTS")?;
            c.change_property32(PropMode::REPLACE, window, motif, motif, &[2, 0, 0, 0, 0])
                .ok()?;
            // Above universe's window, and minimized with it.
            if let Some(parent) = self.parent {
                c.change_property32(
                    PropMode::REPLACE,
                    window,
                    AtomEnum::WM_TRANSIENT_FOR,
                    AtomEnum::WINDOW,
                    &[parent],
                )
                .ok()?;
            }
            // Out of the task bar and the pager: one application, not two.
            let state = self.atom("_NET_WM_STATE")?;
            let taskbar = self.atom("_NET_WM_STATE_SKIP_TASKBAR")?;
            let pager = self.atom("_NET_WM_STATE_SKIP_PAGER")?;
            let asked = ClientMessageEvent::new(32, window, state, [ADD, taskbar, pager, 1, 0]);
            c.send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                asked,
            )
            .ok()?;
            c.flush().ok()
        }

        /// The browser's window: of its process, shown, and the largest,
        /// among those the window manager lists, else the screen's own.
        fn find(&self) -> Option<Window> {
            let c = &self.connection;
            let listed = self.atom("_NET_CLIENT_LIST").and_then(|list| {
                let reply = c
                    .get_property(false, self.root, list, AtomEnum::WINDOW, 0, 4096)
                    .ok()?
                    .reply()
                    .ok()?;
                let windows: Vec<Window> = reply.value32()?.collect();
                (!windows.is_empty()).then_some(windows)
            });
            let windows = match listed {
                Some(windows) => windows,
                None => c.query_tree(self.root).ok()?.reply().ok()?.children,
            };
            let pid_atom = self.atom("_NET_WM_PID")?;
            windows
                .into_iter()
                .filter(|&w| {
                    c.get_property(false, w, pid_atom, AtomEnum::CARDINAL, 0, 1)
                        .ok()
                        .and_then(|cookie| cookie.reply().ok())
                        .and_then(|reply| reply.value32().and_then(|mut v| v.next()))
                        == Some(self.pid)
                })
                .filter(|&w| {
                    c.get_window_attributes(w)
                        .ok()
                        .and_then(|cookie| cookie.reply().ok())
                        .is_some_and(|a| a.map_state != MapState::UNMAPPED)
                })
                .max_by_key(|&w| {
                    c.get_geometry(w)
                        .ok()
                        .and_then(|cookie| cookie.reply().ok())
                        .map_or(0, |g| u32::from(g.width) * u32::from(g.height))
                })
        }

        fn atom(&self, name: &str) -> Option<u32> {
            Some(
                self.connection
                    .intern_atom(false, name.as_bytes())
                    .ok()?
                    .reply()
                    .ok()?
                    .atom,
            )
        }
    }
}

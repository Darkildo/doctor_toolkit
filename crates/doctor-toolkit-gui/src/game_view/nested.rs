//! The game on an X server of its own (Linux and other X11 systems).
//!
//! **Xvfb** when it is installed: a server with its screen in memory only,
//! it needs no display of the host (works under Wayland too) and draws
//! nothing anywhere. Otherwise **Xephyr** with `-parent` on a window of the
//! host display that is never mapped — the bench's way (`tools/game-bench.sh`):
//! nothing appears on the desktop either, but a host X display (X11 or
//! XWayland) is needed. Both are asked for a free display number with
//! `-displayfd 1` and read the frames and take input over the X protocol
//! ([`super::x11`]).
//!
//! The game is started in a process group of its own; stopping sends
//! SIGTERM to the group, SIGKILL after [`KILL_AFTER`], and the X server
//! goes after the game, taking every window left on it.

use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use doctor_toolkit::play::{self, LaunchPlan, Session, SessionState};

/// How long the game may take to end after SIGTERM.
pub const KILL_AFTER: Duration = Duration::from_secs(4);

/// How long an X server may take to start.
const SERVER_START: Duration = Duration::from_secs(15);

/// The X server to run the game on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Server {
    Xvfb(PathBuf),
    Xephyr(PathBuf),
    /// A program and its arguments that prints a display number on its
    /// first line (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    Custom(OsString, Vec<OsString>),
}

impl Server {
    /// `DOCTOR_TOOLKIT_NESTED_X` (a path or name of `Xvfb` or `Xephyr`),
    /// else `Xvfb`, else `Xephyr` from `PATH`.
    pub fn find() -> Option<Self> {
        let from_name = |path: PathBuf| {
            let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
            Some(if name.contains("xvfb") {
                Self::Xvfb(path)
            } else {
                Self::Xephyr(path)
            })
        };
        if let Some(chosen) = std::env::var_os("DOCTOR_TOOLKIT_NESTED_X") {
            let path = PathBuf::from(&chosen);
            let path = if path.components().count() > 1 {
                path
            } else {
                play::find_program(&chosen.to_string_lossy())?
            };
            return from_name(path);
        }
        play::find_program("Xvfb")
            .map(Self::Xvfb)
            .or_else(|| play::find_program("Xephyr").map(Self::Xephyr))
    }

    /// The program's name, for messages.
    pub fn name(&self) -> String {
        match self {
            Self::Xvfb(_) => "Xvfb".into(),
            Self::Xephyr(_) => "Xephyr".into(),
            Self::Custom(program, _) => program.to_string_lossy().into_owned(),
        }
    }

    fn command(&self, size: [u32; 2], parent: Option<u32>) -> Command {
        let [w, h] = size;
        let common = ["-displayfd", "1", "-nolisten", "tcp", "-noreset"];
        match self {
            Self::Xvfb(path) => {
                let mut c = Command::new(path);
                c.args(common)
                    .args(["-screen", "0", &format!("{w}x{h}x24")]);
                c
            }
            Self::Xephyr(path) => {
                let mut c = Command::new(path);
                c.args(common).arg("-no-host-grab");
                // Before `-screen`: after it Xephyr makes a second screen for
                // the parent and shows the first in a window of the host.
                if let Some(parent) = parent {
                    c.args(["-parent", &parent.to_string()]);
                }
                c.args(["-screen", &format!("{w}x{h}")]);
                c
            }
            Self::Custom(program, args) => {
                let mut c = Command::new(program);
                c.args(args);
                c
            }
        }
    }
}

/// A running X server for the game.
pub struct NestedDisplay {
    child: Child,
    number: u32,
    size: [u32; 2],
    /// The unmapped host window Xephyr draws into; it lives as long as the
    /// connection that made it.
    _parent: Option<super::x11::HiddenWindow>,
    server: String,
}

impl NestedDisplay {
    /// Starts `server` with a screen of `size` pixels and waits until it
    /// takes clients.
    pub fn start(server: &Server, size: [u32; 2]) -> Result<Self, String> {
        let parent = match server {
            Server::Xephyr(_) => Some(super::x11::HiddenWindow::create(size)?),
            _ => None,
        };
        let mut command = server.command(size, parent.as_ref().map(|p| p.id()));
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        // The server goes when the editor goes, also when the editor is
        // killed or crashes and no destructor runs: without its X server the
        // game under Wine ends too. (Before this a game left running in an
        // invisible Xvfb after the editor was closed.) The signal follows
        // the thread that starts the server: the UI thread, which lives as
        // long as the editor.
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt as _;
            // SAFETY: only a prctl system call between fork and exec.
            unsafe {
                command.pre_exec(|| {
                    rustix::process::set_parent_process_death_signal(Some(
                        rustix::process::Signal::KILL,
                    ))
                    .map_err(std::io::Error::from)
                });
            }
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("{}: {e}", server.name()))?;
        let stdout = child.stdout.take().expect("piped");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = tx.send(line);
        });
        let number = match rx.recv_timeout(SERVER_START) {
            Ok(line) => line.trim().parse::<u32>().ok(),
            Err(_) => None,
        };
        let Some(number) = number else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{} did not report a display within {} s",
                server.name(),
                SERVER_START.as_secs()
            ));
        };
        doctor_toolkit::report::log(&format!(
            "game tab: {} on :{number}, {}x{}",
            server.name(),
            size[0],
            size[1]
        ));
        Ok(Self {
            child,
            number,
            size,
            _parent: parent,
            server: server.name(),
        })
    }

    /// `:N`, the value of `DISPLAY` for the game.
    pub fn name(&self) -> String {
        format!(":{}", self.number)
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    /// Whether the server still runs.
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Stops the server.
    pub fn stop(&mut self) {
        if self.alive() {
            signal_group(self.child.id(), rustix::process::Signal::TERM);
            let until = Instant::now() + Duration::from_secs(2);
            while self.alive() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

impl Drop for NestedDisplay {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Sends `signal` to the process group led by `pid`.
fn signal_group(pid: u32, signal: rustix::process::Signal) -> bool {
    i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_some_and(|pid| rustix::process::kill_process_group(pid, signal).is_ok())
}

/// The game started on a [`NestedDisplay`].
pub struct TabGame {
    display: Option<NestedDisplay>,
    pub session: Session,
    /// Process group of the launched program (its process id).
    group: u32,
    /// When SIGTERM was sent.
    stopping: Option<Instant>,
    killed: bool,
}

impl TabGame {
    /// Starts `server` and `plan` on it. The plan must not be detached: a
    /// launcher that hands the game to a running client (Steam, Lutris)
    /// would show it on that client's display.
    pub fn start(plan: &LaunchPlan, server: &Server, size: [u32; 2]) -> Result<Self, String> {
        if plan.detached {
            return Err("this launch method starts the game through another program".into());
        }
        let display = NestedDisplay::start(server, size)?;
        let mut command = play::command(plan);
        command
            .env("DISPLAY", display.name())
            // Wine's Wayland driver would take a Wayland display first.
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .process_group(0);
        let child = play::spawn(plan, &mut command).map_err(|e| e.to_string())?;
        let group = child.id();
        Ok(Self {
            display: Some(display),
            session: Session::adopt(child, false),
            group,
            stopping: None,
            killed: false,
        })
    }

    pub fn display(&self) -> Option<&NestedDisplay> {
        self.display.as_ref()
    }

    /// Asks the game to end (SIGTERM to its process group); [`Self::poll`]
    /// kills it after [`KILL_AFTER`].
    pub fn stop(&mut self) {
        if self.stopping.is_none() {
            self.stopping = Some(Instant::now());
            signal_group(self.group, rustix::process::Signal::TERM);
        }
    }

    pub fn stopping(&self) -> bool {
        self.stopping.is_some()
    }

    /// The state of the game after a process scan that found `running`;
    /// once it exited the X server is stopped too.
    pub fn poll(&mut self, running: &[u32]) -> SessionState {
        if let Some(at) = self.stopping
            && !self.killed
            && at.elapsed() > KILL_AFTER
        {
            self.killed = true;
            signal_group(self.group, rustix::process::Signal::KILL);
        }
        // The game's own processes: those of other games on the machine do
        // not keep this one alive once it was stopped.
        let mine: Vec<u32> = if self.stopping.is_some() {
            Vec::new()
        } else {
            running.to_vec()
        };
        let mut state = self.session.poll(&mine);
        // The X server died (or was closed): the game cannot go on.
        if !matches!(state, SessionState::Exited | SessionState::NotStarted(_))
            && self.display.as_mut().is_some_and(|d| !d.alive())
        {
            self.stop();
            state = SessionState::Starting;
        }
        if matches!(state, SessionState::Exited | SessionState::NotStarted(_)) {
            // Whatever is left of the group (Wine's helpers) goes too.
            signal_group(self.group, rustix::process::Signal::KILL);
            if let Some(mut display) = self.display.take() {
                display.stop();
            }
        }
        state
    }
}

impl Drop for TabGame {
    fn drop(&mut self) {
        signal_group(self.group, rustix::process::Signal::KILL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_server(number: u32) -> Server {
        Server::Custom(
            "sh".into(),
            vec!["-c".into(), format!("echo {number}; exec sleep 60").into()],
        )
    }

    fn plan(script: &str) -> LaunchPlan {
        LaunchPlan {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            env: vec![("DT_TEST".into(), "1".into())],
            dir: std::env::temp_dir(),
            detached: false,
        }
    }

    fn wait_for(game: &mut TabGame, done: impl Fn(&SessionState) -> bool) -> SessionState {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            let state = game.poll(&[]);
            if done(&state) || Instant::now() > until {
                return state;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn the_game_gets_the_display_and_is_stopped_with_its_helpers() {
        let dir = std::env::temp_dir().join(format!("dt-tab-game-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("env.txt");
        let helper = dir.join("helper.pid");
        // The "game" records its display, leaves a helper behind (as Wine
        // leaves its services) and runs until stopped.
        let script = format!(
            "echo \"$DISPLAY $DT_TEST $WAYLAND_DISPLAY\" > '{}'; sleep 60 & echo $! > '{}'; exec sleep 60",
            out.display(),
            helper.display()
        );
        let mut game = TabGame::start(&plan(&script), &fake_server(57), [640, 360]).unwrap();
        assert_eq!(game.display().unwrap().name(), ":57");
        assert_eq!(game.display().unwrap().size(), [640, 360]);
        let state = wait_for(&mut game, |_| helper.is_file() && out.is_file());
        assert!(matches!(state, SessionState::Running(_)), "{state:?}");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(std::fs::read_to_string(&out).unwrap().trim(), ":57 1");
        let helper_pid: i32 = std::fs::read_to_string(&helper)
            .unwrap()
            .trim()
            .parse()
            .unwrap();

        game.stop();
        assert!(game.stopping());
        let state = wait_for(&mut game, |s| matches!(s, SessionState::Exited));
        assert_eq!(state, SessionState::Exited);
        assert!(game.display().is_none(), "the server goes with the game");
        // The helper of the same group is gone too.
        let pid = rustix::process::Pid::from_raw(helper_pid).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while rustix::process::test_kill_process(pid).is_ok() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        let status =
            std::fs::read_to_string(format!("/proc/{helper_pid}/stat")).unwrap_or_default();
        assert!(
            status.is_empty() || status.contains(") Z "),
            "helper still runs: {status}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_game_that_exits_takes_its_server_along() {
        let mut game = TabGame::start(&plan("exit 0"), &fake_server(58), [320, 200]).unwrap();
        let state = wait_for(&mut game, |s| matches!(s, SessionState::Exited));
        assert_eq!(state, SessionState::Exited);
        assert!(game.display().is_none());
    }

    #[test]
    fn a_game_that_ignores_sigterm_is_killed() {
        let mut game = TabGame::start(
            &plan("trap '' TERM; sleep 60 & wait; sleep 60"),
            &fake_server(59),
            [320, 200],
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        game.stop();
        let started = Instant::now();
        let state = wait_for(&mut game, |s| matches!(s, SessionState::Exited));
        assert_eq!(state, SessionState::Exited);
        assert!(started.elapsed() >= KILL_AFTER - Duration::from_millis(200));
    }

    /// The editor killed or crashed: no destructor runs, the server must
    /// still go (else the game lives on in an invisible X server).
    #[cfg(target_os = "linux")]
    #[test]
    fn the_server_goes_with_the_thread_that_started_it() {
        let pid = std::thread::spawn(|| {
            let display = NestedDisplay::start(&fake_server(61), [320, 200]).unwrap();
            let pid = display.child.id();
            std::mem::forget(display);
            pid
        })
        .join()
        .unwrap();
        // Killed by the kernel: a zombie (nobody waits for it) or gone.
        let dead = || {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .map(|s| {
                    s.rsplit_once(") ")
                        .is_some_and(|(_, rest)| rest.starts_with('Z'))
                })
                .unwrap_or(true)
        };
        let until = Instant::now() + Duration::from_secs(10);
        while !dead() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            dead(),
            "the server {pid} outlived the thread that started it"
        );
    }

    #[test]
    fn a_server_that_does_not_start_is_reported() {
        let server = Server::Custom("sh".into(), vec!["-c".into(), "echo nonsense".into()]);
        let error = TabGame::start(&plan("exit 0"), &server, [320, 200])
            .err()
            .unwrap();
        assert!(error.contains("did not report a display"), "{error}");
        let mut detached = plan("exit 0");
        detached.detached = true;
        assert!(TabGame::start(&detached, &fake_server(60), [320, 200]).is_err());
    }

    #[test]
    fn server_commands() {
        let args = |s: &Server, parent| {
            let c = s.command([1280, 720], parent);
            c.get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(
            args(&Server::Xvfb("/usr/bin/Xvfb".into()), None),
            "-displayfd 1 -nolisten tcp -noreset -screen 0 1280x720x24"
        );
        assert_eq!(
            args(&Server::Xephyr("Xephyr".into()), Some(42)),
            "-displayfd 1 -nolisten tcp -noreset -no-host-grab -parent 42 -screen 1280x720"
        );
    }
}

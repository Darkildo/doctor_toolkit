//! The listening side: `127.0.0.1:PORT`, one thread per client (at most a
//! few), Hello first.

use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::sync_channel;
use std::time::{Duration, Instant};

use doctor_toolkit_live::config::Config;
use doctor_toolkit_live::{
    ErrorCode, Hello, PROTOCOL_VERSION, Request, Response, kind, read_frame, supported,
};

use crate::{Job, MODULE_VERSION, edition_state, frames, log, snapshot, submit};

/// Clients served at once.
const MAX_CLIENTS: u32 = 4;
/// How long a request may wait for the main thread. The game draws 1–2
/// frames a second under software rendering, and a loading screen can hold
/// the loop longer.
const MAIN_THREAD_WAIT: Duration = Duration::from_secs(20);

static CLIENTS: AtomicU32 = AtomicU32::new(0);

pub fn run(config: Config) {
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, config.port);
    let listener = match TcpListener::bind(address) {
        Ok(l) => l,
        Err(e) => {
            log(&format!("cannot listen on {address}: {e}"));
            return;
        }
    };
    log(&format!("listening on {address}"));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if CLIENTS.load(Ordering::Relaxed) >= MAX_CLIENTS {
            continue;
        }
        CLIENTS.fetch_add(1, Ordering::Relaxed);
        let config = config.clone();
        let spawned = std::thread::Builder::new()
            .name("doctor_live client".into())
            .spawn(move || {
                let _ = std::panic::catch_unwind(move || serve(stream, &config));
                CLIENTS.fetch_sub(1, Ordering::Relaxed);
            });
        if spawned.is_err() {
            CLIENTS.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

fn serve(mut stream: TcpStream, config: &Config) {
    let _ = stream.set_nodelay(true);
    let mut greeted = false;
    loop {
        let Ok((header, payload)) = read_frame(&mut stream) else {
            return;
        };
        if !supported(header.version) {
            let reply = Response::error(
                ErrorCode::Version,
                format!(
                    "the module speaks protocol 1 to {PROTOCOL_VERSION}, the request is version {}",
                    header.version
                ),
            );
            let _ = std::io::Write::write_all(&mut stream, &reply.to_frame(header.kind, header.id));
            return;
        }
        let reply = match Request::decode(header.kind, &payload) {
            Err(e) => Response::error(ErrorCode::BadRequest, e.to_string()),
            Ok(r) if r.since() > header.version => Response::error(
                ErrorCode::Unsupported,
                format!(
                    "this request needs protocol {}, the frame is version {}",
                    r.since(),
                    header.version
                ),
            ),
            Ok(Request::Hello { token, client }) => {
                if !config.token.is_empty() && token != config.token {
                    log(&format!("refused {client}: wrong token"));
                    Response::error(ErrorCode::Denied, "wrong token")
                } else {
                    greeted = true;
                    let (ok, text) = match edition_state() {
                        Ok(()) => (true, doctor_toolkit_live::edition::NAME.to_owned()),
                        Err(e) => (false, e),
                    };
                    Response::Hello(Hello {
                        protocol: PROTOCOL_VERSION,
                        module: MODULE_VERSION.to_owned(),
                        edition_ok: ok,
                        edition: text,
                        writes: config.writes,
                    })
                }
            }
            Ok(_) if !greeted => Response::error(ErrorCode::Denied, "send Hello first"),
            Ok(request) => answer(request, config, header.version),
        };
        let kind = if header.kind == 0 {
            kind::HELLO
        } else {
            header.kind
        };
        if std::io::Write::write_all(
            &mut stream,
            &reply.to_frame_of(header.version, kind, header.id),
        )
        .is_err()
        {
            return;
        }
    }
}

fn answer(request: Request, config: &Config, version: u16) -> Response {
    if let Err(e) = edition_state() {
        return Response::error(ErrorCode::Edition, e);
    }
    if request.writes() && !config.writes {
        return Response::error(
            ErrorCode::Denied,
            "writes are switched off in doctor_live.cfg",
        );
    }
    // Replies from the snapshot of the last frame.
    match request {
        Request::Status => {
            return match snapshot() {
                Some(s) => Response::Status(s.status()),
                None => Response::error(
                    ErrorCode::NotReady,
                    format!("no frame of the main loop yet ({} so far)", frames()),
                ),
            };
        }
        Request::Player => return Response::Player(snapshot().and_then(|s| s.player)),
        Request::Camera => return Response::Camera(snapshot().and_then(|s| s.camera)),
        // The journal is answered from its ring at once.
        Request::Events { after, kinds, max } => {
            let kinds = doctor_toolkit_live::journal::mask::for_version(kinds, version);
            return Response::Events(crate::journal::events(after, kinds, max));
        }
        // Debugging: state the hooks read (stage 35).
        Request::Break { rules, hold_s } => return crate::debug::set_rules(rules, hold_s),
        Request::Control(c) => return crate::debug::control(c),
        Request::Debug => return Response::Debug(crate::debug::debug_state()),
        Request::SetCamera { camera } => return crate::debug::set_camera(camera),
        // The files the game opens (stage 46): counters of the hooks.
        Request::Files => return Response::Files(crate::files::state()),
        // The streams composed from resource patches (stage 100), and a
        // record of the game's table of resources: checked reads.
        Request::Resources => return Response::Resources(crate::resources::state()),
        Request::Resource { ids } => return Response::Resource(crate::resources::loaded(ids)),
        // The journal of edits of the world (stage 47) is the module's own.
        Request::ObjectEdits => return crate::edit::edits(),
        Request::ObjectForget { guid } => return crate::edit::forget(guid),
        // The profile (stage 58): counters of the hooks, read here; the
        // sampler runs on this thread too.
        Request::Profile {
            reset,
            memory,
            sample_ms,
        } => return crate::profile::answer(reset, memory, sample_ms),
        _ => {}
    }
    let (tx, rx) = sync_channel(1);
    let job = Job {
        request,
        deadline: Instant::now() + MAIN_THREAD_WAIT,
        reply: tx,
    };
    if !submit(job) {
        return Response::error(ErrorCode::Failed, "the job queue is closed");
    }
    match rx.recv_timeout(MAIN_THREAD_WAIT + Duration::from_secs(1)) {
        Ok(r) => r,
        Err(_) => Response::error(
            ErrorCode::Timeout,
            "the game's main loop did not come round (paused, minimised or loading)",
        ),
    }
}

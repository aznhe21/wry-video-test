use std::time::{Duration, SystemTime, UNIX_EPOCH};

use image::RgbImage;
use rusttype::{Font, Scale};
use wry::application::event::{Event, StartCause, WindowEvent};
use wry::application::event_loop::{ControlFlow, EventLoop};
use wry::application::window::WindowBuilder;
use wry::http::status::StatusCode;
use wry::http::{Request, Response};
use wry::webview::WebViewBuilder;

const WHITE: image::Rgb<u8> = image::Rgb([0, 0, 0]);

static STARTUP_HTML: &str = include_str!("startup.html");
static FONT: &[u8] = include_bytes!("DejaVuSans.ttf");

// 30fps
const SERVE_INTERVAL: Duration = Duration::from_millis(1000 / 30);
const FRAME_WIDTH: u32 = 1920;
const FRAME_HEIGHT: u32 = 1080;
// const FRAME_WIDTH: u32 = 1280;
// const FRAME_HEIGHT: u32 = 720;
const FONT_HEIGHT: f32 = 64.0;

#[derive(Debug)]
struct GeneratedFrame {
    timestamp: u64,
    width: u32,
    height: u32,
    data: Vec<u8>, // RGB
}

#[derive(Debug, serde::Serialize)]
struct Frame<'a> {
    timestamp: u64,
    send_timestamp: u64,
    width: u32,
    height: u32,
    #[serde(with = "serde_bytes")]
    data: &'a [u8], // RGB
}

fn generate_frames(tx: crossbeam_channel::Sender<GeneratedFrame>) {
    let font = Font::try_from_vec(FONT.to_vec()).unwrap();

    let start = SystemTime::now();
    let mut next_ts = start + SERVE_INTERVAL;
    let mut frame = RgbImage::new(FRAME_WIDTH, FRAME_HEIGHT);

    loop {
        // generate a frame
        frame.fill(255);

        let timestamp = next_ts.duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        imageproc::drawing::draw_text_mut(
            &mut frame,
            WHITE,
            0,
            0,
            Scale {
                x: FONT_HEIGHT * 2.0,
                y: FONT_HEIGHT,
            },
            &font,
            &*format!("rust: {timestamp}"),
        );

        let gen = GeneratedFrame {
            timestamp,
            width: FRAME_WIDTH,
            height: FRAME_HEIGHT,
            data: frame.as_raw().clone(),
        };

        // sleep until next target timestamp
        if let Ok(sleep_dur) = next_ts.duration_since(SystemTime::now()) {
            std::thread::sleep(sleep_dur);
        }

        // send data by channel
        if let Err(crossbeam_channel::TrySendError::Disconnected(_)) = tx.try_send(gen) {
            break;
        }

        next_ts += SERVE_INTERVAL;
    }
}

fn custom_protocol_handler(
    frame_rx: &crossbeam_channel::Receiver<GeneratedFrame>,
    request: &Request<Vec<u8>>,
) -> wry::Result<Response<Vec<u8>>> {
    // eprintln!("ipc: {}", request.uri().path());
    match request.uri().path() {
        "/" => Response::builder().body(STARTUP_HTML.as_bytes().to_vec()),
        "/now" => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            Response::builder().body(serde_cbor::to_vec(&now).unwrap())
        }
        "/frames" => match frame_rx.recv() {
            Ok(gen) => {
                let data = serde_cbor::to_vec(&Frame {
                    send_timestamp: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64,
                    timestamp: gen.timestamp,
                    width: gen.width,
                    height: gen.height,
                    data: &*gen.data,
                })
                .unwrap();
                Response::builder().body(data)
            }
            Err(_) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Vec::new()),
        },
        _ => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Vec::new()),
    }
    .map_err(Into::into)
}

fn main() -> wry::Result<()> {
    let (frame_tx, frame_rx) = crossbeam_channel::bounded(0);

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title(env!("CARGO_PKG_NAME"))
        .build(&event_loop)?;
    let webview = WebViewBuilder::new(window)?
        .with_devtools(true)
        .with_navigation_handler(|url| url.starts_with("https://test.localhost/"))
        .with_new_window_req_handler(|_| false)
        .with_custom_protocol("test".to_string(), move |req| {
            custom_protocol_handler(&frame_rx, req)
        })
        .with_url("test://localhost/")?;

    let _webview = webview.build()?;

    let mut frame_tx = Some(frame_tx);
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => {
                let frame_tx = frame_tx.take().unwrap();
                let _ = std::thread::spawn(move || generate_frames(frame_tx));
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            _ => {}
        }
    })
}

#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(async_fn_in_trait)]

extern crate alloc;
use core::{mem::MaybeUninit, str::from_utf8, ops::Add};
use alloc::format;
use embassy_executor::Executor;
use embassy_net::{Config, Stack, StackResources, tcp::client::{TcpClient, TcpClientState}, dns::DnsSocket};
use embassy_sync::{channel::Channel, mutex::Mutex, blocking_mutex::raw::NoopRawMutex};
use embassy_time::{Timer, Duration};
use embedded_svc::wifi::{Configuration, ClientConfiguration, Wifi};
use esp_backtrace as _;
use esp_println::println;
use hal::{clock::ClockControl, peripherals::Peripherals, prelude::*, IO, timer::TimerGroup, embassy, gpio::{Output, PushPull, Gpio4, Gpio3}};

use esp_wifi::{initialize, EspWifiInitFor, wifi::{WifiMode, WifiController, WifiState, WifiEvent, WifiDevice}};
use hal::{systimer::SystemTimer, Rng};
use reqwless::client::{TlsConfig, HttpClient, TlsVerify};
use picoserve::{routing::{NoPathParameters, get, post, ParsePathSegment}, Router, extract::State, response::{IntoResponse, Response, StatusCode}};
use static_cell::make_static;
const WEB_TASK_POOL_SIZE: usize = 8;

const SSID: &str = env!("SSID");
const PASSWORD: &str = env!("PASSWORD");


// macro_rules! singleton {
//     ($val:expr) => {{
//         type T = impl Sized;
//         static STATIC_CELL: StaticCell<T> = StaticCell::new();
//         let (x,) = STATIC_CELL.init(($val,));
//         x
//     }};
// }

#[global_allocator]
static ALLOCATOR: esp_alloc::EspHeap = esp_alloc::EspHeap::empty();

fn init_heap() {
    const HEAP_SIZE: usize = 32 * 1024;
    static mut HEAP: MaybeUninit<[u8; HEAP_SIZE]> = MaybeUninit::uninit();

    unsafe {
        ALLOCATOR.init(HEAP.as_mut_ptr() as *mut u8, HEAP_SIZE);
    }
}

#[embassy_executor::task]
async fn blink_green(mut pin: Gpio4<Output<PushPull>>) {
    loop {
        pin.toggle().unwrap();
        // delay.delay_ms(500u32);
        Timer::after(Duration::from_millis(2000)).await;
    }
}

#[embassy_executor::task]
async fn blink_red(mut pin: Gpio3<Output<PushPull>>) {
    loop {
        pin.toggle().unwrap();
        // delay.delay_ms(500u32);
        Timer::after(Duration::from_millis(4000)).await;
    }
}

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    println!("start connection task");
    println!("Device capabilities: {:?}", controller.get_capabilities());
    loop {
        match esp_wifi::wifi::get_wifi_state() {
            WifiState::StaConnected => {
                // wait until we're no longer connected
                controller.wait_for_event(WifiEvent::StaDisconnected).await;
                Timer::after(Duration::from_millis(5000)).await
            }
            _ => {}
        }
        if !matches!(controller.is_started(), Ok(true)) {
            let client_config = Configuration::Client(ClientConfiguration {
                ssid: SSID.into(),
                password: PASSWORD.into(),
                ..Default::default()
            });
            controller.set_configuration(&client_config).unwrap();
            println!("Starting wifi");
            controller.start().await.unwrap();
            println!("Wifi started!");
        }
        println!("About to connect...");

        match controller.connect().await {
            Ok(_) => println!("Wifi connected!"),
            Err(e) => {
                println!("Failed to connect to wifi: {e:?}");
                Timer::after(Duration::from_millis(5000)).await
            }
        }
    }
}

#[embassy_executor::task]
async fn net_task(stack: &'static Stack<WifiDevice<'static>>) {
    stack.run().await
}

#[derive(Clone)]
struct AppState {
    counter: i64
}

struct EmbassyTimer;

impl picoserve::Timer for EmbassyTimer {
    type Duration = embassy_time::Duration;
    type TimeoutError = embassy_time::TimeoutError;

    async fn run_with_timeout<F: core::future::Future>(
        &mut self,
        duration: Self::Duration,
        future: F,
    ) -> Result<F::Output, Self::TimeoutError> {
        embassy_time::with_timeout(duration, future).await
    }
}



#[embassy_executor::task]
async fn task(stack: &'static Stack<WifiDevice<'static>>) {
    let mut rx_buffer = [0; 8192];
    let mut tls_read_buffer = [0; 8192];
    let mut tls_write_buffer = [0; 8192];

    loop {
        if stack.is_link_up() {
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    println!("Waiting to get IP address...");
    loop {
        if let Some(config) = stack.config_v4() {
            println!("Got IP: {}", config.address);
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    loop {
        let client_state = TcpClientState::<1,1024,1024>::new();
        let tcp_client = TcpClient::new(&stack, &client_state);
        let dns = DnsSocket::new(&stack);
        let tls_config = TlsConfig::new(123456778_u64, &mut tls_read_buffer, &mut tls_write_buffer, TlsVerify::None);
        let mut http_client = HttpClient::new_with_tls(&tcp_client, &dns, tls_config);
        let mut request = http_client.request(reqwless::request::Method::GET, "https://google.com").await.unwrap();

        let response = request.send(&mut rx_buffer).await.unwrap();
        println!("Http result: {:?}",response.status);

        let body = from_utf8(response.body().read_to_end().await.unwrap()).unwrap();
        println!("Http body: {}",body);

        Timer::after(Duration::from_millis(3000)).await;
    }
}


#[embassy_executor::task]
async fn web_task(
    id: usize,
    stack: &'static Stack<WifiDevice<'static>>,
    // app: &'static picoserve::Router<AppRouter, AppState>,
    config: &'static picoserve::Config<Duration>,
    state: &'static AppState,
) -> ! {


    loop {
        if stack.is_link_up() {
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    println!("Waiting to get IP address...");
    loop {
        if let Some(config) = stack.config_v4() {
            println!("Got IP: {}", config.address);
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    let mut rx_buffer = [0; 1024];
    let mut tx_buffer = [0; 1024];

    let app = picoserve::Router::new()
        .route("/", get(get_counter))
        .route("/increment", post(increment_counter))
        ;

    loop {

        let mut socket = embassy_net::tcp::TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        log::info!("{id}: Listening on TCP:80...");
        if let Err(e) = socket.accept(80).await {
            log::warn!("{id}: accept error: {:?}", e);
            continue;
        }

        log::info!(
            "{id}: Received connection from {:?}",
            socket.remote_endpoint()
        );




        let (socket_rx, socket_tx) = socket.split();
        let pico = picoserve::serve_with_state(
                &app,
                EmbassyTimer,
                config,
                &mut [0; 2048],
                socket_rx,
                socket_tx,
                state,
        ).await;
        match pico
        {
            Ok(handled_requests_count) => {
                log::info!(
                    "{handled_requests_count} requests handled from {:?}",
                    socket.remote_endpoint()
                );
            }
            Err(err) => log::error!("{err:?}"),
        }
    }
}

async fn get_counter(State( state): State<AppState>) -> impl IntoResponse {
    let formatted = format!("Count: {}",state.counter);
    let response: heapless::String<20> = formatted.as_str().into();
    response

}

async fn increment_counter(State(mut state): State<AppState>) -> impl IntoResponse {
    // let counter = state.counter;
    // state.counter += 1;
    // counter.add(1);
    // *state.counter+=1;
    // picoserve::response::StatusCode::new(200)
    "ok"
}

async fn other(path: ParsePathSegment<&str>) -> impl IntoResponse {
    "ok"
}

#[entry]
fn main()->! {
    init_heap();
    let peripherals = Peripherals::take();
    let mut system = peripherals.SYSTEM.split();
    let clocks = ClockControl::max(system.clock_control).freeze();
    // let mut delay = Delay::new(&clocks);
    // embassy_executor
    // setup logger
    // To change the log_level change the env section in .cargo/config.toml
    // or remove it and set ESP_LOGLEVEL manually before running cargo run
    // this requires a clean rebuild because of https://github.com/rust-lang/cargo/issues/10358
    esp_println::logger::init_logger_from_env();
    log::info!("Logger is setup");
    println!("Hello world!");

    let io = IO::new(peripherals.GPIO,peripherals.IO_MUX);
    let pin3 = io.pins.gpio3.into_push_pull_output();
    let pin4 = io.pins.gpio4.into_push_pull_output();
    // let l = Executor::n
    // let executor = EXECUTOR.init(embassy_executor::raw::());
    
    let timer_group = TimerGroup::new(peripherals.TIMG0, &clocks, &mut system.peripheral_clock_control);    

    let timer = SystemTimer::new(peripherals.SYSTIMER).alarm0;
    let init = initialize(
        EspWifiInitFor::Wifi,
        timer,
        Rng::new(peripherals.RNG),
        system.radio_clock_control,
        &clocks,
    )    .unwrap();



    let (wifi, ..) = peripherals.RADIO.split();
    let (wifi_interface, controller) =
        esp_wifi::wifi::new_with_mode(&init, wifi, WifiMode::Sta).unwrap();


    let config = Config::dhcpv4(Default::default());
    let seed = 1234; // very random, very secure seed

    // Init network stack
    let stack = &*make_static!(Stack::new(
        wifi_interface,
        config,
        make_static!(StackResources::<3>::new()),
        seed
    ));
    
    // let app: Router<_,(),NoPathParameters> =
    //     picoserve::Router::new().route("/", get(|| async { "Hello World" }));


    // let aaa: Router<PathRouter> = picoserve::Router::new()
    // .route("/", get(|| async move { "Hello World" }));

    // let app = &*singleton!(picoserve::Router::new()
    //     .route("/", get(|| async move { "Hello World" }))
    //     .route(
    //         ("/set", parse_path_segment()),
    //         get(
    //             |led_is_on, State(SharedControl(control)): State::<SharedControl>| async move {
    //                 control.lock().await.gpio_set(0, led_is_on).await;
    //                 DebugValue(led_is_on)
    //             }
    //         )
    //     ));

    let executor = make_static!(Executor::new());

    

    // let app: &mut Router<_,(),NoPathParameters> = make_static!(picoserve::Router::new()
    //     .route("/", get(|| async move { "Hello World" })
    // ));

    let config = &*make_static!(picoserve::Config {
        start_read_request_timeout: Some(Duration::from_secs(5)),
        read_request_timeout: Some(Duration::from_secs(1)),
    });

    let channel: Channel<NoopRawMutex,i64,2> = Channel::new();

    let l = channel.receiver();
    let l3 = channel.receiver();

    let state = make_static!(AppState{ counter: 0_i64});

    embassy::init(&clocks,timer_group.timer0);
    executor.run(|spawner| {
        spawner.spawn(blink_green(pin4)).unwrap();
        spawner.spawn(blink_red(pin3)).unwrap();
        spawner.spawn(connection(controller)).unwrap();
        spawner.spawn(net_task(stack)).unwrap();
        // spawner.spawn(task(&stack)).unwrap();
        spawner.spawn(web_task(1,&stack,&config, state)).unwrap();
    })
}

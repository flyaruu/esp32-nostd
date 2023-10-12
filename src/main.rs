#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

extern crate alloc;
use core::{mem::MaybeUninit, str::from_utf8};
use embassy_executor::Executor;
use embassy_net::{Config, Stack, StackResources, tcp::client::{TcpClient, TcpClientState}, dns::DnsSocket};
use embassy_time::{Timer, Duration};
use embedded_svc::wifi::{Configuration, ClientConfiguration, Wifi};
use esp_backtrace as _;
use esp_println::println;
use hal::{clock::ClockControl, peripherals::Peripherals, prelude::*, Delay, IO, timer::TimerGroup, embassy, gpio::{Output, PushPull, Gpio4, Gpio3}};

use esp_wifi::{initialize, EspWifiInitFor, wifi::{WifiMode, WifiController, WifiState, WifiEvent, WifiDevice}};

use hal::{systimer::SystemTimer, Rng};
use reqwless::client::HttpClient;
use static_cell::StaticCell;


const SSID: &str = env!("SSID");
const PASSWORD: &str = env!("PASSWORD");

macro_rules! singleton {
    ($val:expr) => {{
        type T = impl Sized;
        static STATIC_CELL: StaticCell<T> = StaticCell::new();
        let (x,) = STATIC_CELL.init(($val,));
        x
    }};
}

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
        println!("Loop...");
        pin.toggle().unwrap();
        // delay.delay_ms(500u32);
        Timer::after(Duration::from_millis(200)).await;
    }
}

#[embassy_executor::task]
async fn blink_red(mut pin: Gpio3<Output<PushPull>>) {
    loop {
        println!("Loop...");
        pin.toggle().unwrap();
        // delay.delay_ms(500u32);
        Timer::after(Duration::from_millis(330)).await;
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

#[embassy_executor::task]
async fn task(stack: &'static Stack<WifiDevice<'static>>) {
    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

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
        let client_state = TcpClientState::<4,4096,4096>::new();
        let tcp_client = TcpClient::new(&stack, &client_state);
        let dns = DnsSocket::new(&stack);
        let mut http_client = HttpClient::new(&tcp_client, &dns);
        let mut request = http_client.request(reqwless::request::Method::GET, "http://10.11.12.112:8000/version").await.unwrap();

        let response = request.send(&mut rx_buffer).await.unwrap();
        println!("Http result: {:?}",response.status);

        let body = from_utf8(response.body().read_to_end().await.unwrap()).unwrap();
        println!("Http body: {}",body);

        Timer::after(Duration::from_millis(3000)).await;
    }
}

#[entry]
fn main() -> ! {
    init_heap();
    let peripherals = Peripherals::take();
    let mut system = peripherals.SYSTEM.split();
    let clocks = ClockControl::max(system.clock_control).freeze();
    // let mut delay = Delay::new(&clocks);

    static EXECUTOR: StaticCell<Executor> = StaticCell::new();

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

    let executor = EXECUTOR.init(Executor::new());
    
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
    let stack = &*singleton!(Stack::new(
        wifi_interface,
        config,
        singleton!(StackResources::<3>::new()),
        seed
    ));

    embassy::init(&clocks,timer_group.timer0);

    executor.run(|spawner| {
        spawner.spawn(blink_green(pin4)).unwrap();
        spawner.spawn(blink_red(pin3)).unwrap();
        spawner.spawn(connection(controller)).unwrap();
        spawner.spawn(net_task(stack)).unwrap();
        spawner.spawn(task(&stack)).unwrap();
    });


   


}

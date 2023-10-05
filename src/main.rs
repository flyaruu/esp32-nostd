#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

extern crate alloc;
use core::mem::MaybeUninit;
use embassy_executor::Executor;
use embassy_time::{Timer, Duration};
use esp_backtrace as _;
use esp_println::println;
use hal::{clock::ClockControl, peripherals::Peripherals, prelude::*, Delay, IO, timer::TimerGroup, embassy, gpio::{Output, PushPull, Gpio4, Gpio3}};

use esp_wifi::{initialize, EspWifiInitFor};

use hal::{systimer::SystemTimer, Rng};
use static_cell::StaticCell;
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
    embassy::init(&clocks,timer_group.timer0);

    executor.run(|spawner| {
        spawner.spawn(blink_green(pin4)).unwrap();
        spawner.spawn(blink_red(pin3)).unwrap();

    });


    // let timer = SystemTimer::new(peripherals.SYSTIMER).alarm0;
    // let _init = initialize(
    //     EspWifiInitFor::Wifi,
    //     timer,
    //     Rng::new(peripherals.RNG),
    //     system.radio_clock_control,
    //     &clocks,
    // )
    // .unwrap();




}

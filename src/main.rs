use crate::socket::to_dect210;
use crate::weather::calculate_cycles_needed_blocked;
use core::time::Duration;
use std::thread;
use uom::si::f32::Time;
use uom::si::time::{hour, minute, second};

mod duration;
mod location;
mod socket;
mod weather;

fn main() {
    println!("Querying weather...");
    let mut client = socket::get_client();
    let mut devices = client.list_devices().unwrap();
    let dev = devices.first_mut().unwrap();

    println!("   Temperature (Current): {} °C", to_dect210(dev).celsius);

    thread::sleep(core::time::Duration::from_secs(2));
    let n_cycles = calculate_cycles_needed_blocked(location::BERLIN);

    println!("{n_cycles} cycles needed");

    for i in 1..=n_cycles {
        println!("Started watering cycle {i}...");

        println!("Turned electricity ON...");
        dev.turn_on(&mut client).expect("Failed to turn on ");

        let pump_interval = Time::new::<second>(60.0);
        println!("Pumping for {} seconds...", pump_interval.get::<second>());
        thread::sleep(Duration::from_secs(pump_interval.value as u64));

        let shutdown_interval = Time::new::<second>(2.0);
        println!(
            "Pumping completed! Waiting for {} seconds to shut the pump off...",
            shutdown_interval.get::<second>()
        );
        thread::sleep(Duration::from_secs(shutdown_interval.get::<second>() as u64));

        println!("Turned electricity OFF...");
        dev.turn_off(&mut client).expect("Failed to turn off ");

        if i < n_cycles {
            let sleep_interval = Time::new::<minute>(60.0) - (pump_interval + shutdown_interval);
            println!(
                "Waiting for the next cycle in {}...",
                sleep_interval.get::<hour>().round()
            );
            thread::sleep(Duration::from_secs(sleep_interval.get::<second>() as u64));
        }

        println!("Ended watering cycle {i}...");
    }
}

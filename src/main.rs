use crate::weather::calculate_cycles_needed_blocked;
use core::time::Duration;
use std::thread;
use uom::si::f32::Time;
use uom::si::time::{hour, minute, second};

mod duration;
mod location;
mod socket;
mod weather;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Querying weather...");
    let mut client = socket::FritzClient::login()?;
    let dev = client.configured_device()?;

    println!("   Device: {} ({})", dev.name, dev.product);
    println!("   Temperature (Current): {:.1} °C", dev.celsius);

    if !dev.switchable {
        return Err(format!(
            "device '{}' (AIN {}) is not a switchable socket",
            dev.name, dev.ain
        )
        .into());
    }
    if !dev.present {
        return Err(format!(
            "device '{}' is offline — check that the FRITZ!Smart Energy plug is \
             plugged in and connected to the FRITZ!Box",
            dev.name
        )
        .into());
    }

    thread::sleep(core::time::Duration::from_secs(1));

    // Ensure the device is turned off to trigger before running the program.
    client.turn_off(&dev.ain)?;

    thread::sleep(core::time::Duration::from_secs(1));

    let n_cycles = calculate_cycles_needed_blocked(location::BERLIN)?;

    println!("{n_cycles} cycles needed");

    for i in 1..=n_cycles {
        println!("Started watering cycle {i}...");

        println!("Turned electricity ON...");
        client.turn_on(&dev.ain)?;

        let pump_interval = Time::new::<second>(60.0);
        println!("Pumping for {} seconds...", pump_interval.get::<second>());
        thread::sleep(Duration::from_secs(pump_interval.value as u64));

        let shutdown_interval = Time::new::<second>(2.0);
        println!(
            "Pumping completed! Waiting for {} seconds to shut the pump off...",
            shutdown_interval.get::<second>()
        );
        thread::sleep(Duration::from_secs(shutdown_interval.get::<second>() as u64));

        if let Err(err) = client.turn_off(&dev.ain) {
            eprintln!("!!! CRITICAL: failed to switch the pump OFF: {err}");
            eprintln!("!!! The pump may still be running — switch it off manually now.");
            return Err(err.into());
        }
        println!("Turned electricity OFF...");

        if i < n_cycles {
            let sleep_interval = Time::new::<minute>(60.0) - (pump_interval + shutdown_interval);
            println!(
                "Waiting for the next cycle in {} hour(s)...",
                sleep_interval.get::<hour>().round()
            );
            thread::sleep(Duration::from_secs(sleep_interval.get::<second>() as u64));
        }

        println!("Ended watering cycle {i}...");
    }

    Ok(())
}

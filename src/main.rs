use crate::socket::to_dect210;
use crate::weather::calculate_cycles_needed_blocked;
use chrono::Local;
use std::thread;

mod duration;
mod location;
mod socket;
mod weather;


fn main() {
    println!("Starting...");

    let mut client = socket::get_client();
    let mut devices = client.list_devices().unwrap();
    let dev = devices.first_mut().unwrap();

    println!("Temperature {}°C", to_dect210(dev).celsius);

    thread::sleep(core::time::Duration::from_secs(2));
    let n = calculate_cycles_needed_blocked();


    for i in 1..=n {
        println!("Started watering Cycle {i} at {}...", Local::now().format("%Y-%m-%d %H:%M:%S"));

        dev.turn_on(&mut client).expect("Failed to turn on ");
        thread::sleep(duration::seconds(65));

        dev.turn_off(&mut client).expect("Failed to turn off ");
        thread::sleep(duration::minutes(30));

        println!("Ended watering Cycle {i} at {}...", Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}



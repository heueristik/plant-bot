use fritzapi::{AVMDevice, FritzClient, FritzDect2XX};
use std::env;

pub fn get_client() -> FritzClient {
    dotenv::dotenv().ok();

    let username: String = env::var("FRITZ_USERNAME")
        .expect("FRITZ_USERNAME not found in .env")
        .parse()
        .unwrap();

    let password: String = env::var("FRITZ_PASSWORD")
        .expect("FRITZ_PASSWORD not found in .env")
        .parse()
        .unwrap();

    FritzClient::new(username, password)
}

pub fn to_dect210(dev: &mut AVMDevice) -> &mut FritzDect2XX {
    match dev {
        AVMDevice::FritzDect2XX(dect210) => Some(dect210),
        AVMDevice::Other(_) => None,
    }
    .unwrap()
}

#[test]
fn test_device() {
    use std::thread;
    let mut client = get_client();

    let mut devices = client.list_devices().unwrap();

    let dev = devices.first_mut().unwrap();

    for _ in 0..5 {
        dev.turn_off(&mut client).unwrap();
        thread::sleep(std::time::Duration::from_secs(1));

        dev.turn_on(&mut client).unwrap();
        thread::sleep(std::time::Duration::from_secs(1));
    }
}

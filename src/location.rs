use open_meteo_api::models::TimeZone;

pub struct Location {
    pub latitude: f32,
    pub longitude: f32,
    pub time_zone: TimeZone,
}

pub const BERLIN: Location = Location {
    latitude: 52.489_95,
    longitude: 13.349_879,
    time_zone: TimeZone::EuropeBerlin,
};

#[allow(dead_code)]
pub const ROME: Location = Location {
    latitude: 41.88,
    longitude: 12.50,
    time_zone: TimeZone::EuropeBerlin,
};

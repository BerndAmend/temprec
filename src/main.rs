extern crate regex;

use std::io::prelude::*;
use std::fs::File;
use regex::Regex;

#[derive(Debug)]
enum Temperature {
    MiliCelcius(i16),
    ReadError,
    SensorError,
}

struct Sensor {
    path: String,
}

impl Sensor {
    fn new(sensorid: &str) -> Self {
        Sensor {
            path: format!("/sys/bus/w1/devices/{}/w1_slave", sensorid),
        }
    }

    fn read_temp(&self) -> Temperature {
        let mut f = match File::open(&self.path) {
            Ok(d) => d,
            Err(_) => return Temperature::ReadError,
        };
        let mut s = String::new();
        match f.read_to_string(&mut s) {
            Ok(_) => (),
            Err(_) => return Temperature::ReadError,
        };

        Temperature::MiliCelcius(42)

        // check crc
        /*if !re.match(r"([0-9a-f]{2} ){9}: crc=[0-9a-f]{2} YES", zeile) {
            return Temperature::SensorError;
        }

        if let Some(m) = re.match(r"([0-9a-f]{2} ){9}t=([+-]?[0-9]+)", zeile) {
            Temperature::MiliCelcius(float(m.group(2))/1000)
        } else {
            Temperature::SensorError
        }*/
    }
}



fn main() {
    let sensor = Sensor::new("bla");
    println!("Temperature {:?}", sensor.read_temp());
}

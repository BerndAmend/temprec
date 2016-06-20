extern crate regex;
extern crate time;

use std::io::BufReader;
use std::io::prelude::*;
use std::fs::File;
use regex::Regex;
use std::fs::OpenOptions;

#[derive(Debug,PartialEq,Eq)]
enum Temperature {
    MiliCelcius(i16),
    ReadError,
    SensorError,
}

impl Temperature {
    fn has_changed(&self, other: &Temperature) -> bool {
        match self {
            &Temperature::MiliCelcius(temp1) => match other {
                &Temperature::MiliCelcius(temp2) => (temp1 - temp2).abs() > 100,
                _ => self != other,
            },
            _ => self != other,
        }
    }
}

struct Sensor {
    path: String,
    crc_regex: Regex,
    cap_regex: Regex,
}

impl Sensor {
    fn new(sensorid: &str) -> Self {
        Sensor {
            path: format!("/sys/bus/w1/devices/{}/w1_slave", sensorid),
            crc_regex: Regex::new(r"([0-9a-f]{2} ){9}: crc=[0-9a-f]{2} YES").unwrap(),
            cap_regex: Regex::new(r"([0-9a-f]{2} ){9}t=([+-]?[0-9]+)").unwrap(),
        }
    }

    fn read_temp(&self) -> Temperature {
        let f = match File::open(&self.path) {
            Ok(d) => d,
            Err(_) => return Temperature::ReadError,
        };
        let f = BufReader::new(f);

        let mut lines = f.lines();

        // check crc
        let line = lines.next().unwrap().unwrap();
        if !self.crc_regex.is_match(&line) {
            return Temperature::SensorError;
        }

        let line = lines.next().unwrap().unwrap();
        if let Some(cap) = self.cap_regex.captures(&line) {
            Temperature::MiliCelcius(cap.at(2).unwrap().parse::<i16>().unwrap())
        } else {
            Temperature::SensorError
        }
    }
}


fn main() {
    let sensor = Sensor::new("28-000003a4c40d");

    let mut file = OpenOptions::new().create(true).append(true).open("temperatures.csv").unwrap();

    let mut last_temp = Temperature::ReadError;
    loop {
        let current_temp = sensor.read_temp();
        if current_temp.has_changed(&last_temp) {
            file.write_all(match current_temp {
                Temperature::MiliCelcius(temp) => format!("{},{}\n", time::now_utc().rfc3339(), temp),
                Temperature::ReadError => format!("{},read_error\n", time::now_utc().rfc3339()),
                Temperature::SensorError => format!("{},sensor_error\n", time::now_utc().rfc3339()),
            }.as_bytes()).unwrap();
            last_temp = current_temp;
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
}

#![feature(plugin, custom_derive)]
#![plugin(rocket_codegen)]

extern crate regex;
extern crate time;
extern crate rocket;
extern crate dht22_pi;

use std::io;
use std::path::{Path, PathBuf};

use rocket::State;
use rocket::response::NamedFile;

use std::collections::BTreeMap;
use std::io::BufReader;
use std::io::prelude::*;
use std::fs;
use std::fs::{File, OpenOptions};
use regex::Regex;

use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use std::env;

macro_rules! try_return(
    ($e:expr) => {{
        match $e {
            Ok(v) => v,
            Err(e) => { println!("Error: {}", e); return; }
        }
    }}
);

#[derive(Debug,PartialEq,Eq,Clone)]
enum Temperature {
    Invalid,
    MiliCelcius(i16),
    Error(String),
}

impl Temperature {
    fn has_changed(&self, other: &Temperature) -> bool {
        match *self {
            Temperature::MiliCelcius(temp1) => {
                match *other {
                    Temperature::MiliCelcius(temp2) => (temp1 - temp2).abs() > 200,
                    _ => self != other,
                }
            }
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
            path: Self::get_sensor_filename(sensorid),
            crc_regex: Regex::new(r"([0-9a-f]{2} ){9}: crc=[0-9a-f]{2} YES").unwrap(),
            cap_regex: Regex::new(r"([0-9a-f]{2} ){9}t=([+-]?[0-9]+)").unwrap(),
        }
    }

    fn read_temp(&self) -> Temperature {
        let f = match File::open(&self.path) {
            Ok(d) => d,
            Err(_) => return Temperature::Error(format!("Couldn't open file {}", &self.path)),
        };
        let f = BufReader::new(f);

        let mut lines = f.lines();

        // check crc
        if let Some(Ok(line)) = lines.next() {
            if !self.crc_regex.is_match(&line) {
                return Temperature::Error(format!("crc failed line=\"{}\"", &line));
            }
        } else {
            return Temperature::Error("crc line couldn't be read".to_owned());
        }

        // read temperature
        if let Some(Ok(line)) = lines.next() {
            if let Some(cap) = self.cap_regex.captures(&line) {
                if let Some(temp_string) = cap.get(2) {
                    let temp_string = temp_string.as_str();
                    match temp_string.parse::<i16>() {
                        Ok(temp) => Temperature::MiliCelcius(temp),
                        Err(ref err) => {
                            Temperature::Error(format!("Couldn't parse number string=\"{}\" \
                                                        err=\"{}\"",
                                                       &temp_string,
                                                       &err))
                        }
                    }
                } else {
                    Temperature::Error(format!("Regular expression value 2 was invalid \"{}\"",
                                               &line))
                }
            } else {
                Temperature::Error(format!("Couldn't parse temperature line \"{}\"", &line))
            }
        } else {
            Temperature::Error("temperature line couldn't be read".to_owned())
        }
    }

    fn get_sensor_filename(sensorid: &str) -> String {
        format!("{}/{}/w1_slave", Self::get_sensor_base_path(), sensorid)
    }

    fn get_sensor_base_path() -> String {
        if cfg!(target_arch = "x86_64") {
            format!("{}/mnt/sys/bus/w1/devices/",
                    env::home_dir().unwrap().display())
        } else {
            "/sys/bus/w1/devices/".to_owned()
        }
    }

    fn get_all_sensor_ids() -> Vec<String> {
        let mut ret = vec![];
        let paths = std::fs::read_dir(Self::get_sensor_base_path()).unwrap();

        for path in paths {
            let path = path.unwrap().path().file_name().unwrap().to_str().unwrap().to_owned();
            if path.starts_with("28-") {
                ret.push(path);
            }
        }
        ret
    }
}

type SensorStoreDataType = (time::Tm, Temperature);
type SensorStoreType = Vec<SensorStoreDataType>;

struct SensorStore {
    sensorid: String,
    data: Arc<RwLock<SensorStoreType>>,
}

impl SensorStore {
    fn new(sensorid: &str) -> Self {
        let data = Arc::new(RwLock::new(vec![]));
        {
            let sensorid = sensorid.to_owned();
            let data = data.clone();
            thread::spawn(move || {
                let sensor = Sensor::new(&sensorid);

                let filename = SensorStore::get_filename(&sensorid);

                // load existing data
                if let Some(d) = SensorStore::read_from_file(&filename) {
                    println!("Read existing data for sensor {}", sensorid);
                    *data.write().unwrap() = d;
                }

                let mut last_temp = Temperature::Invalid;
                loop {
                    let current_temp = sensor.read_temp();
                    if current_temp.has_changed(&last_temp) {
                        let d = (time::now_utc(), current_temp.clone());
                        let mut data = data.write().unwrap();
                        SensorStore::append_to_file(&filename, &d);
                        data.push(d);
                        last_temp = current_temp;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(5));
                }
            });
        }
        SensorStore {
            sensorid: sensorid.to_owned(),
            data: data,
        }
    }

    fn get_filename(sensorid: &str) -> String {
        format!("{}.csv", sensorid)
    }

    fn append_to_file(filename: &str, d: &SensorStoreDataType) {
        let mut file = OpenOptions::new().create(true).append(true).open(&filename).unwrap();
        try_return!(file.write_all(format!("{}\n", SensorStore::as_csv_line(d)).as_bytes()));
    }

    fn read_from_file(filename: &str) -> Option<SensorStoreType> {
        if let Ok(file) = File::open(filename) {
            let file = BufReader::new(file);
            Some(file.lines()
                .filter_map(|x| x.ok())
                .map(|s| {
                    let splitted: Vec<&str> = s.split(',').collect();
                    if splitted.len() != 2 {
                        println!("skip invalid line line=\"{}\"", s);
                        (time::now_utc(), Temperature::Invalid)
                    } else if let Ok(time) = time::strptime(splitted[0], "%Y-%m-%dT%H:%M:%SZ") {
                        let temp = match splitted[1].parse::<i16>() {
                            Ok(temp) => Temperature::MiliCelcius(temp),
                            Err(_) => Temperature::Error(splitted[1].to_owned()),
                        };
                        (time, temp)
                    } else {
                        println!("skip line with invalid time line=\"{}\"", s);
                        (time::now_utc(), Temperature::Invalid)
                    }
                })
                .filter(|x| match *x {
                    (_, Temperature::Invalid) => false,
                    _ => true,
                })
                .collect())
        } else {
            None
        }
    }

    fn as_csv_line(data: &SensorStoreDataType) -> String {
        let &(time, ref temp) = data;
        match *temp {
            Temperature::MiliCelcius(ref temp) => format!("{},{}", time.rfc3339(), &temp),
            Temperature::Error(ref err) => format!("{},{}", time.rfc3339(), &err),
            Temperature::Invalid => format!("{},invalid", time.rfc3339()),
        }
    }

    fn as_csv_internal(data: &SensorStoreType) -> String {
        data.iter().map(SensorStore::as_csv_line).collect::<Vec<String>>().join("\n")
    }

    fn as_csv(&self) -> String {
        let data = self.data.read().unwrap();
        SensorStore::as_csv_internal(&data)
    }

    fn as_csv_from(&self, from: &time::Tm) -> String {
        let data = self.data.read().unwrap();
        data.iter()
            .rev()
            .take_while(|&&(ref time, _)| time > from)
            .map(SensorStore::as_csv_line)
            .collect::<Vec<String>>()
            .into_iter()
            .rev()
            .skip(1)
            .collect::<Vec<String>>()
            .join("\n")
    }

    fn remove(&mut self, t: &time::Tm) {
        //println!("remove {:?}", t);
        let mut data = self.data.write().unwrap();
        data.retain(|&(tm, _)| tm != *t);
        let filename = SensorStore::get_filename(&self.sensorid);
        fs::rename(&filename, format!("{}.bak", &filename)).unwrap();
        let mut file =
            OpenOptions::new().create(true).truncate(true).write(true).open(&filename).unwrap();
        try_return!(file.write_all(SensorStore::as_csv_internal(&data).as_bytes()));
        try_return!(file.write_all(b"\n"));
    }
}

struct Sensors {
    sensors: BTreeMap<String, SensorStore>
}

type MSensors = Mutex<Sensors>;

impl Sensors {
    fn all() -> Sensors {
        Sensors {
            sensors: Sensor::get_all_sensor_ids()
                        .iter()
                        .map(|id| (id.to_owned(), SensorStore::new(id)))
                        .collect()
        }
    }
}

#[get("/")]
fn index() -> io::Result<NamedFile> {
    NamedFile::open("content/index.html")
}

#[get("/<file..>", rank = 2)]
fn files(file: PathBuf) -> Option<NamedFile> {
    NamedFile::open(Path::new("content/").join(file)).ok()
}

#[get("/api/get/sensors")]
fn sensor_list(sensors: State<MSensors>) -> String {
    sensors.lock().unwrap()
        .sensors.iter()
        .map(|(id, _)| id.to_owned())
        .collect::<Vec<String>>()
        .join("\n")
}

#[get("/api/get/<id>")]
fn get_temp(id: &str, sensors: State<MSensors>) -> Option<String> {
    let sensors = sensors.lock().unwrap();
    let sensors = &sensors.sensors;

    if let Some(sensor) = sensors.get(id) {
        Some(sensor.as_csv())
    } else {
        None
    }
}

#[get("/api/get/<id>/<from>")]
fn get_temp_from(id: &str, from: &str, sensors: State<MSensors>) -> Option<String> {
    let sensors = sensors.lock().unwrap();
    if let Some(sensor) = sensors.sensors.get(id) {
        if let Ok(t) = time::strptime(from, "%Y-%m-%dT%H:%M:%SZ") {
            Some(sensor.as_csv_from(&t))
        } else {
            None
        }
    } else {
        None
    }
}

#[get("/api/remove/<id>/<time>")]
fn remove_temp(id: &str, time: &str, sensors: State<MSensors>) -> Option<String> {
    let mut sensors = sensors.lock().unwrap();
    if let Some(sensor) = sensors.sensors.get_mut(id) {
        if let Ok(t) = time::strptime(time, "%Y-%m-%dT%H:%M:%SZ") {
            sensor.remove(&t);
            Some(sensor.as_csv())
        } else {
            None
        }
    } else {
        None
    }
}

fn main() {
    println!("Start temprec");

    let result = dht22_pi::read(17);
    println!("intial read={:?}", result);

    let sensors = Sensors::all();
    for id in sensors.sensors.keys() {
        println!(" * {}", id);
    }

    rocket::ignite()
        .manage(Mutex::new(sensors))
        .mount("/", routes![index, sensor_list, get_temp, get_temp_from, remove_temp, files])
        .launch();
}

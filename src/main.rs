use std::io;
use std::path::{Path, PathBuf};

use rocket::response::NamedFile;

use regex::Regex;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::prelude::*;
use std::io::BufReader;

use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use std::env;

use chrono::prelude::*;

#[derive(Debug, PartialEq, Eq, Clone)]
enum Temperature {
    Invalid,
    MiliCelcius(i32),
    Error(String),
}

impl Temperature {
    fn has_changed(&self, other: &Temperature) -> bool {
        match *self {
            Temperature::MiliCelcius(temp1) => match *other {
                Temperature::MiliCelcius(temp2) => (temp1 - temp2).abs() > 200,
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
    fn new(id: &str) -> Self {
        Sensor {
            path: Self::get_sensor_filename(id),
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
                    match temp_string.parse::<i32>() {
                        Ok(temp) => {
                            if temp >= -55_000 && temp <= 125_000 {
                                Temperature::MiliCelcius(temp)
                            } else {
                                Temperature::Error(format!(
                                    "Measured temperature {} is outside of sensor range",
                                    temp
                                ))
                            }
                        }
                        Err(ref err) => Temperature::Error(format!(
                            "Couldn't parse number string=\"{}\" \
                             err=\"{}\"",
                            &temp_string, &err
                        )),
                    }
                } else {
                    Temperature::Error(format!(
                        "Regular expression value 2 was invalid \"{}\"",
                        &line
                    ))
                }
            } else {
                Temperature::Error(format!("Couldn't parse temperature line \"{}\"", &line))
            }
        } else {
            Temperature::Error("temperature line couldn't be read".to_owned())
        }
    }

    fn get_sensor_filename(id: &str) -> String {
        format!("{}/{}/w1_slave", Self::get_sensor_base_path(), id)
    }

    fn get_sensor_base_path() -> String {
        if cfg!(target_arch = "x86_64") {
            format!(
                "{}/mnt/sys/bus/w1/devices/",
                env::home_dir().unwrap().display()
            )
        } else {
            "/sys/bus/w1/devices/".to_owned()
        }
    }

    fn get_all_sensor_ids() -> Vec<String> {
        let mut ret = vec![];
        let paths = std::fs::read_dir(Self::get_sensor_base_path()).unwrap();

        for path in paths {
            let path = path
                .unwrap()
                .path()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            if path.starts_with("28-") {
                ret.push(path);
            }
        }
        ret
    }
}

struct Measurement {
    time: chrono::DateTime<Utc>,
    temp: Temperature,
}

impl Measurement {
    fn new(time: chrono::DateTime<Utc>, temp: Temperature) -> Measurement {
        Measurement {
            time: time,
            temp: temp,
        }
    }

    fn as_csv_line(&self) -> String {
        match self.temp {
            Temperature::MiliCelcius(ref temp) => format!("{},{}\n", self.time.rfc3339(), &temp),
            Temperature::Error(ref err) => format!("{},{}\n", self.time.rfc3339(), &err),
            Temperature::Invalid => format!("{},invalid\n", self.time.rfc3339()),
        }
    }
}

struct SensorStore {
    id: String,
    data: Arc<RwLock<Vec<Measurement>>>,
}

impl SensorStore {
    fn new(id: &str) -> Self {
        let data = Arc::new(RwLock::new(vec![]));
        {
            let id = id.to_owned();
            let data = data.clone();
            thread::spawn(move || {
                let sensor = Sensor::new(&id);

                let filename = format!("{}.csv", &id);

                // load existing data
                if let Some(d) = SensorStore::read_from_file(&filename) {
                    println!("Read existing data for sensor {}", id);
                    *data.write().unwrap() = d;
                }

                let mut last_temp = Temperature::Invalid;
                loop {
                    let current_temp = sensor.read_temp();
                    if current_temp.has_changed(&last_temp) {
                        let d = Measurement::new(Utc::now(), current_temp.clone());
                        if let Err(err) = SensorStore::append_to_file(&filename, &d) {
                            println!("Couldn't write sensor measurement err=\"{}\"", err);
                        }
                        data.write().unwrap().push(d);
                        last_temp = current_temp;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(5));
                }
            });
        }
        SensorStore {
            id: id.to_owned(),
            data: data,
        }
    }

    fn append_to_file(filename: &str, d: &Measurement) -> io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&filename)
            .unwrap();
        file.write_all(d.as_csv_line().as_bytes())
    }

    fn read_from_file(filename: &str) -> Option<Vec<Measurement>> {
        if let Ok(file) = File::open(filename) {
            let file = BufReader::new(file);
            Some(
                file.lines()
                    .filter_map(|x| x.ok())
                    .map(|s| {
                        let splitted: Vec<&str> = s.split(',').collect();
                        if splitted.len() != 2 {
                            println!("skip invalid line \"{}\"", s);
                            Measurement::new(Utc::now(), Temperature::Invalid)
                        } else if let Ok(time) =
                            chrono::DateTime::parse_from_str(splitted[0], "%Y-%m-%dT%H:%M:%SZ")
                        {
                            let temp = match splitted[1].parse::<i32>() {
                                Ok(temp) => Temperature::MiliCelcius(temp),
                                Err(_) => Temperature::Error(splitted[1].to_owned()),
                            };
                            Measurement::new(time, temp)
                        } else {
                            println!("skip line with invalid time line=\"{}\"", s);
                            Measurement::new(Utc::now(), Temperature::Invalid)
                        }
                    })
                    .filter(|x| match *x {
                        Measurement {
                            temp: Temperature::Invalid,
                            ..
                        } => false,
                        _ => true,
                    })
                    .collect(),
            )
        } else {
            None
        }
    }

    fn as_csv(&self) -> String {
        self.data
            .read()
            .unwrap()
            .iter()
            .map(Measurement::as_csv_line)
            .collect::<String>()
    }

    fn as_csv_from(&self, from: &chrono::DateTime<Utc>) -> String {
        let data = self.data.read().unwrap();
        data.iter()
            .rev()
            .take_while(|&&Measurement { ref time, .. }| time > from)
            .map(Measurement::as_csv_line)
            .collect::<Vec<String>>()
            .into_iter()
            .rev()
            .skip(1)
            .collect::<String>()
    }
}

struct Sensors {
    sensors: BTreeMap<String, SensorStore>,
}

impl Sensors {
    fn all() -> Sensors {
        Sensors {
            sensors: Sensor::get_all_sensor_ids()
                .iter()
                .map(|id| (id.to_owned(), SensorStore::new(id)))
                .collect(),
        }
    }

    fn get(&self, id: &str) -> Option<&SensorStore> {
        self.sensors.get(id)
    }
}

type State<'a> = rocket::State<'a, Mutex<Sensors>>;

#[get("/")]
fn index() -> io::Result<NamedFile> {
    NamedFile::open("content/index.html")
}

#[get("/<file..>", rank = 2)]
fn files(file: PathBuf) -> Option<NamedFile> {
    NamedFile::open(Path::new("content/").join(file)).ok()
}

#[get("/api/get/sensors")]
fn sensor_list(sensors: State) -> String {
    sensors
        .lock()
        .unwrap()
        .sensors
        .iter()
        .map(|(id, _)| id.to_owned())
        .collect::<Vec<String>>()
        .join("\n")
}

#[get("/api/get/name/<name>")]
fn get_temp_by_name(name: String, state: State) -> Option<String> {
    state.lock().unwrap().get(&name).map(SensorStore::as_csv)
}

#[get("/api/get/id/<id>")]
fn get_temp_by_id(id: usize, state: State) -> Option<String> {
    state
        .lock()
        .unwrap()
        .sensors
        .iter()
        .nth(id)
        .map(|(_, store)| store.as_csv())
}

#[get("/api/get/name/<name>/<from>")]
fn get_temp_from_by_name(name: String, from: String, sensors: State) -> Option<String> {
    if let Ok(t) = time::strptime(&from, "%Y-%m-%dT%H:%M:%SZ") {
        sensors
            .lock()
            .unwrap()
            .sensors
            .get(&name)
            .map(|sensor| sensor.as_csv_from(&t))
    } else {
        None
    }
}

#[get("/api/get/id/<id>/<from>")]
fn get_temp_from_by_id(id: usize, from: String, sensors: State) -> Option<String> {
    if let Ok(t) = time::strptime(&from, "%Y-%m-%dT%H:%M:%SZ") {
        sensors
            .lock()
            .unwrap()
            .sensors
            .iter()
            .nth(id)
            .map(|(_, store)| store.as_csv_from(&t))
    } else {
        None
    }
}

// sudo setcap 'cap_net_bind_service=+ep' target/release/temprec
fn main() {
    println!("Start temprec");

    //let result = dht22_pi::read(17);
    //println!("initial read={:?}", result);

    let sensors = Sensors::all();
    for id in sensors.sensors.keys() {
        println!(" * {}", id);
    }

    rocket::ignite()
        .manage(Mutex::new(sensors))
        .mount(
            "/",
            routes![
                index,
                sensor_list,
                get_temp_by_name,
                get_temp_by_id,
                get_temp_from_by_name,
                get_temp_from_by_id,
                files,
            ],
        )
        .launch();
}

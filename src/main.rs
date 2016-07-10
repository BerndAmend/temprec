extern crate regex;
extern crate time;
extern crate hyper;

use std::collections::BTreeMap;
use std::io::BufReader;
use std::io::prelude::*;
use std::fs;
use std::fs::{File, OpenOptions};
use regex::Regex;

use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use std::io::copy;
use hyper::header::ContentType;
use hyper::server::{Server, Request, Response};
use hyper::{Get, Post};
use hyper::uri::RequestUri::AbsolutePath;
use hyper::mime::{Mime, TopLevel, SubLevel, Attr, Value};

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
            Temperature::MiliCelcius(temp1) => match *other {
                Temperature::MiliCelcius(temp2) => (temp1 - temp2).abs() > 150,
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
                if let Some(temp_string) = cap.at(2) {
                    match temp_string.parse::<i16>() {
                        Ok(temp) => Temperature::MiliCelcius(temp),
                        Err(ref err) => Temperature::Error(format!("Couldn't parse number string=\"{}\" err=\"{}\"", &temp_string, &err)),
                    }
                } else {
                    Temperature::Error(format!("Regular expression value 2 was invalid \"{}\"", &line))
                }
            } else {
                Temperature::Error(format!("Couldn't parse temperature line \"{}\"", &line))
            }
        } else {
            return Temperature::Error("temperature line couldn't be read".to_owned());
        }
    }

    fn get_sensor_filename(sensorid: &str) -> String {
        format!("{}/{}/w1_slave", Self::get_sensor_base_path(), sensorid)
    }

    fn get_sensor_base_path() -> String {
        if cfg!(target_arch = "x86_64") {
            format!("{}/mnt/sys/bus/w1/devices/", env::home_dir().unwrap().display())
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
            data: data
        }
    }

    fn all() -> BTreeMap<String,Self> {
        Sensor::get_all_sensor_ids().iter().map(|id| (id.to_owned(), SensorStore::new(id))).collect()
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
                        } else if let Ok(time) =time::strptime(splitted[0], "%Y-%m-%dT%H:%M:%SZ") {
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
        data.iter().rev()
            .take_while(|&&(ref time, _)| time > from)
            .map(SensorStore::as_csv_line)
            .collect::<Vec<String>>()
            .into_iter().rev().skip(1).collect::<Vec<String>>().join("\n")
    }

    fn remove(&mut self, t: &time::Tm) {
        //println!("remove {:?}", t);
        let mut data = self.data.write().unwrap();
        data.retain(|&(tm,_)| tm != *t);
        let filename = SensorStore::get_filename(&self.sensorid);
        fs::rename(&filename, format!("{}.bak", &filename));
        let mut file = OpenOptions::new().create(true).truncate(true).write(true).open(&filename).unwrap();
        try_return!(file.write_all(SensorStore::as_csv_internal(&data).as_bytes()));
        try_return!(file.write_all(b"\n"));
    }
}

struct SenderHandler {
    sensors: BTreeMap<String, SensorStore>,
}

impl SenderHandler {
    fn parse_arguments<'a>(url: &'a str) -> Vec<(&'a str, &'a str)> {
        let mut ret = vec![];
        let splitted: Vec<&str> = url.split('?').collect();
        if splitted.len() == 2 {
            for query in splitted[1].split('&') {
                let sp: Vec<&str> = query.split('=').collect();
                match splitted.len() {
                    0 => {},
                    1 => ret.push((sp[0], "")),
                    2 => ret.push((sp[0], sp[1])),
                    _ => {},
                }
            }
        }
        ret
    }

    fn handle(&mut self, req: Request, mut res: Response) {
        let uri = req.uri.clone();
        match uri {
            AbsolutePath(ref path) => match (&req.method, &path[..]) {
                (&Get, u) => {
                    let u = match u {
                        "/" => "/index.html",
                        o => o,
                    };
                    if u == "/api/get/sensors" {
                        res.headers_mut().set(
                                ContentType(Mime(TopLevel::Text, SubLevel::Plain,
                                    vec![(Attr::Charset, Value::Utf8)])));
                        let mut res = try_return!(res.start());
                        let ret: String = self.sensors.iter().map(|(id,_)| {
                            id.to_owned()
                        }).collect::<Vec<String>>().join("\n");
                        try_return!(res.write_all(ret.as_bytes()));
                        try_return!(res.end());
                    } else if u.starts_with("/api/remove/temp") {
                        let mut id: Option<&str> = None;
                        let mut t: Option<time::Tm> = None;
                        for (name, value) in SenderHandler::parse_arguments(u) {
                            match name {
                                "id" => id = Some(value),
                                "time" => {
                                    if let Ok(time) =time::strptime(value, "%Y-%m-%dT%H:%M:%SZ") {
                                        t = Some(time);
                                    }
                                },
                                _ => {},
                            }
                        }

                        if let Some(sensor) = self.sensors.get_mut(id.unwrap_or("")) {
                            if let Some(t) = t {
                                res.headers_mut().set(
                                ContentType(Mime(TopLevel::Text, SubLevel::Plain,
                                    vec![(Attr::Charset, Value::Utf8)])));
                                let mut res = try_return!(res.start());
                                sensor.remove(&t);
                                try_return!(res.write_all(sensor.as_csv().as_bytes()));
                                try_return!(res.end());
                            } else {
                                *res.status_mut() = hyper::NotFound;
                            }
                        } else {
                            *res.status_mut() = hyper::NotFound;
                        }
                    } else if u.starts_with("/api/get/temp") {
                        let mut id: Option<&str> = None;
                        let mut from: Option<time::Tm> = None;
                        for (name, value) in SenderHandler::parse_arguments(u) {
                            match name {
                                "id" => id = Some(value),
                                "from" => {
                                    if let Ok(time) =time::strptime(value, "%Y-%m-%dT%H:%M:%SZ") {
                                        from = Some(time);
                                    }
                                },
                                _ => {},
                            }
                        }

                        if let Some(sensor) = self.sensors.get(id.unwrap_or("")) {
                            res.headers_mut().set(
                                ContentType(Mime(TopLevel::Text, SubLevel::Plain,
                                    vec![(Attr::Charset, Value::Utf8)])));
                            let mut res = try_return!(res.start());
                            if let Some(from) = from {
                                // transfer only what was requested
                                try_return!(res.write_all(sensor.as_csv_from(&from).as_bytes()));
                            } else {
                                // transfer everything
                                try_return!(res.write_all(sensor.as_csv().as_bytes()));
                            }
                            try_return!(res.end());
                        } else {
                            *res.status_mut() = hyper::NotFound;
                        }
                    } else {
                        //println!("request: {}", u);
                        match File::open(format!("content/{}", u)) {
                            Ok(mut f) => {
                                let mut res = try_return!(res.start());
                                try_return!(copy(&mut f, &mut res));
                                try_return!(res.end());
                            },
                            Err(e) => {
                                println!("url: {} error: {}", u, e);
                                *res.status_mut() = hyper::NotFound;
                            },
                        }
                    }
                    return;
                },
                (&Post, u) => match u {
                    "/api/set_alias" => {
                    },
                    _ => {
                        *res.status_mut() = hyper::NotFound;
                        return;
                    },
                },
                _ => {
                    *res.status_mut() = hyper::NotFound;
                    return;
                }
            },
            _ => {
                return;
            }
        };
    }
}

fn main() {
    println!("Start temprec");
    let http_handler = Mutex::new(SenderHandler {
        sensors: SensorStore::all(),
    });

    for id in http_handler.lock().unwrap().sensors.keys() {
        println!(" * {}", id);
    }

    let mut http_server = Server::http("0.0.0.0:8080").unwrap();
    http_server.keep_alive(Some(Duration::from_secs(15)));
    http_server.handle(move |req: Request, res: Response| http_handler.lock().unwrap().handle(req, res)).unwrap();
}

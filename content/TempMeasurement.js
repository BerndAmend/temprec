if (!Date.prototype.toRISOString) {
    (function () {

    function pad(number) {
        if (number < 10) {
        return '0' + number;
        }
        return number;
    }

    Date.prototype.toRISOString = function () {
        return this.getUTCFullYear() +
        '-' + pad(this.getUTCMonth() + 1) +
        '-' + pad(this.getUTCDate()) +
        'T' + pad(this.getUTCHours()) +
        ':' + pad(this.getUTCMinutes()) +
        ':' + pad(this.getUTCSeconds()) +
        'Z';
    };

    }());
}

class TempMeasurement {
    constructor() {
    this.data = [];
    this.last_time = null;
    this.interval = null;
    this.req = new XMLHttpRequest();
    this.initial_handler = [];
    this.update_handler = [];
    }

    append(str, send_updates) {
    if (str.length === 0)
        return;
    let splitted = str.split("\n");
    for (let line of splitted) {
        if (line.length == 0)
        continue;
        let d = line.split(",");
        let t = new Date(d[0]);
        let v = parseInt(d[1]);
        if (isNaN(v)) {
        console.error(`${d} error ${d[1]} `);
        } else {
        let new_entry = [t, v / 1000.0];
        this.data.push(new_entry);
        if (send_updates) {
            this.update_handler.forEach(f => f(new_entry))
        }
        this.last_time = t;
        }
    }
    }

    clear() {
    this.data.clear();
    }

    stop() {
    if (this.interval != null) {
        clearInterval(this.interval);
        this.interval = null;
    }
    }

    on_initial(func) {
    if (this.data.length != 0) {
        func(this.data);
    }
    this.initial_handler.push(func);
    }

    on_update(func) {
    this.update_handler.push(func);
    }

    start() {
    this.req.open('GET', '/api/get/id/0', true);
    this.req.onreadystatechange = () => {
        if (this.req.readyState == 4) {
        this.append(this.req.responseText, false);
        this.initial_handler.forEach(f => f(this.data))
        this.interval = setInterval(() => {
            this.req.open('GET', `/api/get/id/0/${this.last_time.toRISOString()}`, true);
            this.req.onreadystatechange = () => {
            if (this.req.readyState == 4) {
                this.append(this.req.responseText, true);
            }
            }
            this.req.send(null);
        }, 5000);
        }
    };
    this.req.send(null);
    }
}

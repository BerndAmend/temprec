#!/bin/sh

export ROCKET_ADDRESS=0.0.0.0
export ROCKET_PORT=80
export RUST_BACKTRACE=1
export PATH="$HOME/.cargo/bin:$PATH"
/home/pi/.cargo/bin/cargo run --release 2> log.txt


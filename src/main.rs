#![allow(dead_code)]
mod config;
mod merge;
mod render;
mod model;
mod sources;
mod theme;
mod waybar;

fn main() {
    println!("{}", r#"{"text":"printbar","tooltip":"","class":["ok"],"alt":"ok"}"#);
}

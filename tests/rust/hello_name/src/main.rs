fn main() {
    if let Some(name) = std::env::args().skip(1).next() {
        println!("Hello, {name}");
    } else {
        println!("Hello, world!");
    }
}

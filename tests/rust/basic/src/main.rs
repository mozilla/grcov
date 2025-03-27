use std::fmt::Debug;

#[derive(Debug)]
pub struct Ciao {
    pub saluto: String,
}

fn main() {
    let ciao = Ciao{ saluto: String::from("salve") };

    assert!(ciao.saluto == "salve");
}

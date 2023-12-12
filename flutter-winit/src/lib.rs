#![deny(warnings)]

mod context;
mod handler;
mod keyboard;
mod pointer;
mod window;

pub use window::FlutterWindow;

#[cfg(test)]
mod tests {
    #[test]
    fn test_link() {
        println!("Linking worked");
    }
}

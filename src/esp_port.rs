
use serialport::{SerialPortType, DataBits, FlowControl, Parity, StopBits};

pub fn find_esp_port() -> Option<String> {
    let ports = serialport::available_ports().ok()?;

    // First pass: look for USB ports that look like ESP.
    for p in &ports {
        if let SerialPortType::UsbPort(usb) = &p.port_type {
            let product = usb.product.as_deref().unwrap_or("").to_lowercase();
            let manufacturer = usb.manufacturer.as_deref().unwrap_or("").to_lowercase();
            if product.contains("esp") || manufacturer.contains("espressif") {
                return Some(p.port_name.clone());
            }
        }
    }

    // Second pass: pick first ttyACM or ttyUSB as a reasonable default on Linux.
    ports
        .into_iter()
        .map(|p| p.port_name)
        .find(|name| name.contains("ttyACM") || name.contains("ttyUSB"))
}
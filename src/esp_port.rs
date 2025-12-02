use serialport::{available_ports, SerialPortType, UsbPortInfo, SerialPort};
use std::io::{self, Write};

pub fn find_esp_port() -> Option<String> {
    let ports = available_ports().ok()?;

    #[cfg(target_os = "linux")]
    {
        for p in &ports {
            if let  SerialPortType::UsbPort(usb) = &p.port_type {
                let product = usb.product.as_deref().unwrap_or("").to_lowercase();
                let manufacturer = usb.manufacturer.as_deref().unwrap_or("").to_lowercase();
                if product.contains("esp") || manufacturer.contains("espressif") {
                    return Some(p.port_name.clone());
                }
            }
        }

        let found =  ports
            .into_iter()
            .map(|p| p.port_name)
            .find(|name| name.contains("ttyUSB") || name.contains("ttyACM"));

        return found
    }

    #[cfg(target_os = "windows")]
    {
        for p in &ports {
            if let SerialPortType::UsbPort(usb) = &p.port_type {
                let product = usb.product.as_deref().unwrap_or("").to_lowercase();
                let manufacturer = usb.manufacturer.as_deref().unwrap_or("").to_lowercase();
                if product.contains("esp") || manufacturer.contains("espressif") {
                    return Some(p.port_name.clone());
                }
            }
        }
        
        let found = ports
            .into_iter()
            .find(|port| port.port_name.eq_ignore_ascii_case("COM4"))
            .map(|port| port.port_name);
    }

    #[allow(unreachable_code)]
    None
}

pub fn send_cli_command(
    port: &mut dyn SerialPort,
    cmd: &str,
) -> io::Result<()> {
    port.write_all(cmd.as_bytes())?;
    port.write_all(b"\r\n")?;
    port.flush()?;
    Ok(())
}

/// Simple test client to send orders to the gateway
use std::io::{Read, Write};
use std::net::TcpStream;

fn main() {
    println!("Connecting to gateway at 127.0.0.1:9000...");
    let mut stream = match TcpStream::connect("127.0.0.1:9000") {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to connect: {}", e);
            return;
        }
    };
    println!("Connected!");

    // Create a simple new order command
    // For now, just send some bytes to test the connection
    let test_data = b"Hello gateway!";
    
    // Send length prefix (4 bytes, little endian)
    let len = (test_data.len() as u32).to_le_bytes();
    if stream.write_all(&len).is_err() || stream.write_all(test_data).is_err() || stream.flush().is_err() {
        eprintln!("Failed to send data");
        return;
    }
    
    println!("Sent {} bytes", test_data.len());
    
    // Try to read response (with timeout)
    if stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).is_err() {
        eprintln!("Failed to set timeout");
        return;
    }
    
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf) {
        Ok(_) => {
            let response_len = u32::from_le_bytes(len_buf);
            println!("Expecting {} byte response", response_len);
            
            let mut response = vec![0u8; response_len as usize];
            if stream.read_exact(&mut response).is_ok() {
                println!("Received: {:?}", response);
            }
        }
        Err(e) => {
            println!("No response received (expected, since we sent invalid data): {}", e);
        }
    }
}
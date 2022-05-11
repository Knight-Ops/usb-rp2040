use std::time::Duration;

use rand;
use rusb::{self, DeviceHandle, GlobalContext};

struct DeviceInformation {
    vid: u16,
    pid: u16,
    in_addr: u8,
    out_addr: u8,
}

fn find_2040() -> Result<DeviceInformation, rusb::Error> {
    for device in rusb::devices()?.iter() {
        let device_description = device.device_descriptor()?;

        if device_description.vendor_id() != 0x2e8a {
            continue;
        }

        if device_description.product_id() != 0x0003 {
            continue;
        }

        let config_count = device_description.num_configurations();

        for idx in 0..config_count {
            let config_descriptor = device.config_descriptor(idx)?;

            for interface in config_descriptor.interfaces() {
                for descriptor in interface.descriptors() {
                    if descriptor.class_code() != 0xFF {
                        continue;
                    }

                    if descriptor.sub_class_code() != 0x00 {
                        continue;
                    }

                    if descriptor.protocol_code() != 0x00 {
                        continue;
                    }

                    // If we made it here, then we found a 2040
                    let mut device_info = DeviceInformation {
                        vid: 0x2e8a,
                        pid: 0x0003,
                        in_addr: 0,
                        out_addr: 0,
                    };

                    for endpoint in descriptor.endpoint_descriptors() {
                        match endpoint.direction() {
                            rusb::Direction::In => device_info.in_addr = endpoint.address(),
                            rusb::Direction::Out => device_info.out_addr = endpoint.address(),
                        }
                    }

                    return Ok(device_info);
                }
            }
        }
    }
    Err(rusb::Error::NoDevice)
}

struct USB2040 {
    handle: DeviceHandle<GlobalContext>,
    device_info: DeviceInformation,
}

impl USB2040 {
    pub fn new(handle: DeviceHandle<GlobalContext>, device_info: DeviceInformation) -> Self {
        USB2040 {
            handle,
            device_info,
        }
    }

    pub fn exclusive_access(&mut self, option: ExclusivityOption) -> Result<usize, rusb::Error> {
        let cmd_id = 0x1;
        let cmd_size = 0x1;
        let transfer_length = 0x0;

        let mut args = [0; 16];

        match option {
            ExclusivityOption::NOT_EXCLUSIVE => args[0] = 0,
            ExclusivityOption::EXCLUSIVE => args[0] = 1,
            ExclusivityOption::EXCLUSIVE_AND_EJECT => args[0] = 2,
        }

        let command = PicobootCommand::new(cmd_id, cmd_size, transfer_length, &args);

        let cmd_slice = unsafe {
            std::slice::from_raw_parts(
                &command as *const _ as *const u8,
                std::mem::size_of::<PicobootCommand>(),
            )
        };

        self.write_out_raw(cmd_slice, Duration::from_millis(5000))
    }

    pub fn reboot(&mut self, dPC: u32, dSP: u32, dDelayMs: u32) -> Result<usize, rusb::Error> {
        let cmd_id = 0x2;
        let cmd_size = 0xc;
        let transfer_length = 0x0;

        let mut args = [0; 16];

        // TODO: I need to implement dPC and dSP validation based on page 175 of the 2040 datasheet
        // https://datasheets.raspberrypi.com/rp2040/rp2040-datasheet.pdf

        let pc_bytes: [u8; 4] = dPC.to_le_bytes();
        let sp_bytes: [u8; 4] = dSP.to_le_bytes();
        let delay_ms: [u8; 4] = dDelayMs.to_le_bytes();

        args[0..4].copy_from_slice(&pc_bytes);
        args[4..8].copy_from_slice(&sp_bytes);
        args[8..12].copy_from_slice(&delay_ms);

        let command = PicobootCommand::new(cmd_id, cmd_size, transfer_length, &args);

        let cmd_slice = unsafe {
            std::slice::from_raw_parts(
                &command as *const _ as *const u8,
                std::mem::size_of::<PicobootCommand>(),
            )
        };

        self.write_out_raw(cmd_slice, Duration::from_millis(5000))
    }

    pub fn flash_erase(&mut self, dAddr: u32, dSize: u32) -> Result<usize, rusb::Error> {
        let cmd_id = 0x3;
        let cmd_size = 0x8;
        let transfer_length = 0x0;

        let mut args = [0; 16];

        if dAddr % 4096 != 0 || dSize % 4096 != 0 {
            return Err(rusb::Error::InvalidParam);
        }

        let addr_bytes: [u8; 4] = dAddr.to_le_bytes();
        let size_bytes: [u8; 4] = dSize.to_le_bytes();

        args[0..4].copy_from_slice(&addr_bytes);
        args[4..8].copy_from_slice(&size_bytes);

        let command = PicobootCommand::new(cmd_id, cmd_size, transfer_length, &args);

        let cmd_slice = unsafe {
            std::slice::from_raw_parts(
                &command as *const _ as *const u8,
                std::mem::size_of::<PicobootCommand>(),
            )
        };

        self.write_out_raw(cmd_slice, Duration::from_millis(5000))
    }

    pub fn read(&mut self, dAddr: u32, dSize: u32) -> Result<usize, rusb::Error> {
        let cmd_id = 0x84;
        let cmd_size = 0x8;
        let transfer_length = dSize;

        let mut args = [0; 16];

        let addr_bytes: [u8; 4] = dAddr.to_le_bytes();
        let size_bytes: [u8; 4] = dSize.to_le_bytes();

        args[0..4].copy_from_slice(&addr_bytes);
        args[4..8].copy_from_slice(&size_bytes);

        let command = PicobootCommand::new(cmd_id, cmd_size, transfer_length, &args);

        let cmd_slice = unsafe {
            std::slice::from_raw_parts(
                &command as *const _ as *const u8,
                std::mem::size_of::<PicobootCommand>(),
            )
        };

        self.write_out_raw(cmd_slice, Duration::from_millis(5000))
    }

    pub fn write(&mut self, dAddr: u32, dSize: u32) -> Result<usize, rusb::Error> {
        let cmd_id = 0x5;
        let cmd_size = 0x8;
        let transfer_length = dSize;

        let mut args = [0; 16];

        // TODO: This should only apply to writing flash
        if dAddr % 256 != 0 || dSize % 256 != 0 {
            return Err(rusb::Error::InvalidParam);
        }

        let addr_bytes: [u8; 4] = dAddr.to_le_bytes();
        let size_bytes: [u8; 4] = dSize.to_le_bytes();

        args[0..4].copy_from_slice(&addr_bytes);
        args[4..8].copy_from_slice(&size_bytes);

        let command = PicobootCommand::new(cmd_id, cmd_size, transfer_length, &args);

        let cmd_slice = unsafe {
            std::slice::from_raw_parts(
                &command as *const _ as *const u8,
                std::mem::size_of::<PicobootCommand>(),
            )
        };

        self.write_out_raw(cmd_slice, Duration::from_millis(5000))
    }

    pub fn exit_xip(&mut self) -> Result<usize, rusb::Error> {
        let cmd_id = 0x6;
        let cmd_size = 0x0;
        let transfer_length = 0x0;

        let mut args = [0; 16];

        let command = PicobootCommand::new(cmd_id, cmd_size, transfer_length, &args);

        let cmd_slice = unsafe {
            std::slice::from_raw_parts(
                &command as *const _ as *const u8,
                std::mem::size_of::<PicobootCommand>(),
            )
        };

        self.write_out_raw(cmd_slice, Duration::from_millis(5000))
    }

    pub fn enter_xip(&mut self) -> Result<usize, rusb::Error> {
        let cmd_id = 0x7;
        let cmd_size = 0x0;
        let transfer_length = 0x0;

        let mut args = [0; 16];

        let command = PicobootCommand::new(cmd_id, cmd_size, transfer_length, &args);

        let cmd_slice = unsafe {
            std::slice::from_raw_parts(
                &command as *const _ as *const u8,
                std::mem::size_of::<PicobootCommand>(),
            )
        };

        self.write_out_raw(cmd_slice, Duration::from_millis(5000))
    }

    pub fn exec(&mut self, dAddr: u32) -> Result<usize, rusb::Error> {
        let cmd_id = 0x8;
        let cmd_size = 0x4;
        let transfer_length = 0x0;

        let mut args = [0; 16];

        let addr_bytes: [u8; 4] = dAddr.to_le_bytes();

        args[0..4].copy_from_slice(&addr_bytes);

        let command = PicobootCommand::new(cmd_id, cmd_size, transfer_length, &args);

        let cmd_slice = unsafe {
            std::slice::from_raw_parts(
                &command as *const _ as *const u8,
                std::mem::size_of::<PicobootCommand>(),
            )
        };

        self.write_out_raw(cmd_slice, Duration::from_millis(5000))
    }

    pub fn vectorized_flash(&mut self, dAddr: u32) -> Result<usize, rusb::Error> {
        let cmd_id = 0x9;
        let cmd_size = 0x4;
        let transfer_length = 0x0;

        let mut args = [0; 16];

        let addr_bytes: [u8; 4] = dAddr.to_le_bytes();

        args[0..4].copy_from_slice(&addr_bytes);

        let command = PicobootCommand::new(cmd_id, cmd_size, transfer_length, &args);

        let cmd_slice = unsafe {
            std::slice::from_raw_parts(
                &command as *const _ as *const u8,
                std::mem::size_of::<PicobootCommand>(),
            )
        };

        self.write_out_raw(cmd_slice, Duration::from_millis(5000))
    }

    fn write_out_raw(&mut self, buf: &[u8], timeout: Duration) -> rusb::Result<usize> {
        self.handle
            .write_bulk(self.device_info.out_addr, buf, timeout)
    }

    fn read_out_raw(self, buf: &mut [u8], timeout: Duration) -> rusb::Result<usize> {
        self.handle
            .read_bulk(self.device_info.out_addr, buf, timeout)
    }

    fn write_in_raw(&mut self, buf: &[u8], timeout: Duration) -> rusb::Result<usize> {
        self.handle
            .write_bulk(self.device_info.in_addr, buf, timeout)
    }

    fn read_in_raw(&self, buf: &mut [u8], timeout: Duration) -> rusb::Result<usize> {
        self.handle
            .read_bulk(self.device_info.in_addr, buf, timeout)
    }
}

#[repr(C)]
struct PicobootCommand {
    magic: u32,
    token: u32,
    cmd_id: u8,
    cmd_size: u8,
    reserved: u16,
    transfer_length: u32,
    args: [u8; 16],
}

impl PicobootCommand {
    fn new(cmd_id: u8, cmd_size: u8, transfer_length: u32, args: &[u8; 16]) -> Self {
        PicobootCommand {
            magic: 0x431fd10,
            token: rand::random::<u32>(),
            cmd_id,
            cmd_size,
            reserved: 0x0000,
            transfer_length,
            args: *args,
        }
    }
}

#[repr(u8)]
enum ExclusivityOption {
    NOT_EXCLUSIVE = 0,
    EXCLUSIVE = 1,
    EXCLUSIVE_AND_EJECT = 2,
}

fn main() {
    let device_info = find_2040().unwrap();

    let mut device_handle = None;

    for device in rusb::devices().unwrap().iter() {
        let device_descriptor = device.device_descriptor().unwrap();

        if device_descriptor.vendor_id() == device_info.vid
            && device_descriptor.product_id() == device_info.pid
        {
            device_handle = Some(device.open().unwrap());
        }
    }

    let mut usb_2040 = if device_handle.is_none() {
        panic!("Unable to find 2040 attached to USB");
    } else {
        USB2040::new(device_handle.unwrap(), device_info)
    };

    usb_2040.reboot(0, 0, 1000).unwrap();
}

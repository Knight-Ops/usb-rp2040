use std::borrow::Borrow;
use std::convert::TryInto;
use std::time::Duration;

use rand;
use rusb::{self, Context, DeviceHandle, GlobalContext, UsbContext};

use std::sync::Mutex;

use lazy_static::lazy_static;
use std::ops::Deref;
use std::marker::PhantomData;

lazy_static! {
    static ref TOKEN: Mutex<u32> = Mutex::new(1);
}

#[derive(Debug)]
pub struct DeviceInformation {
    vid: u16,
    pid: u16,
    in_addr: u8,
    out_addr: u8,
    iface: u8,
    config: u8,
    setting: u8,
}

#[derive(Clone, Copy, Debug)]
#[repr(u32)]
pub enum CommandStatusCode {
    Ok = 0,
    UnknownCommand = 1,
    InvalidCommandLength = 2,
    InvalidTransferLength = 3,
    InvalidAddress = 4,
    BadAlignment = 5,
    InterleavedWrite = 6,
    Rebooting = 7,
    UnknownError = 8,
}

#[derive(Clone, Copy, Debug)]
pub struct CommandStatus {
    dToken: u32,
    dStatusCode: CommandStatusCode,
    bCmdId: u8,
    bInProgress: u8,
    reserved: [u8; 6]
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
                        iface: descriptor.interface_number(),
                        config: config_descriptor.number(),
                        setting: descriptor.setting_number(),
                    };

                    for endpoint in descriptor.endpoint_descriptors() {
                        println!("Endpoint : {:?}", endpoint);
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

pub struct USB2040 {
    handle: DeviceHandle<Context>,
    device_info: DeviceInformation,
}

impl Drop for USB2040 {
    fn drop(&mut self) {
        self.handle
            .release_interface(self.device_info.iface)
            .unwrap();
    }
}

impl USB2040 {
    pub fn new(handle: DeviceHandle<Context>, device_info: DeviceInformation) -> Self {
        let mut usb2040 = USB2040 {
            handle,
            device_info,
        };

        usb2040
            .handle
            .claim_interface(usb2040.device_info.iface)
            .unwrap();
        usb2040.handle.set_active_configuration(usb2040.device_info.config).unwrap();
        usb2040.handle.set_alternate_setting(usb2040.device_info.iface, usb2040.device_info.setting).unwrap();

        usb2040
    }

    pub fn try_find_and_open_2040() -> Result<Self, rusb::Error> {
        let device_info = find_2040().unwrap();
        let mut device_handle = None;
        let ctx = Context::new()?;

        for device in ctx.devices()?.iter() {
            let device_descriptor = device.device_descriptor()?;

            if device_descriptor.vendor_id() == device_info.vid
                && device_descriptor.product_id() == device_info.pid
            {
                device_handle = Some(device.open()?);
            }
        }

        let usb_2040 = if device_handle.is_none() {
            return Err(rusb::Error::NoDevice)
        } else {
            USB2040::new(device_handle.unwrap(), device_info)
        };

        Ok(usb_2040)
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

        self.write_out_cmd(command, None, Duration::from_secs(1))
    }

    /// Reboot the Pi2040, starting execution at the new PC and SP, with a delay of DelayMs
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

        self.write_out_cmd(command, None, Duration::from_secs(1))
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

        self.write_out_cmd(command, None, Duration::from_secs(1))
    }

    pub fn read(&mut self, dAddr: u32, dSize: u32) -> Result<Vec<u8>, rusb::Error> {
        let cmd_id = 0x84;
        let cmd_size = 0x8;
        let transfer_length = dSize;

        let mut args = [0; 16];

        let addr_bytes: [u8; 4] = dAddr.to_le_bytes();
        let size_bytes: [u8; 4] = dSize.to_le_bytes();

        args[0..4].copy_from_slice(&addr_bytes);
        args[4..8].copy_from_slice(&size_bytes);

        let command = PicobootCommand::new(cmd_id, cmd_size, transfer_length, &args);
        let mut read_data: Vec<u8> = vec![0; transfer_length as usize];
        self.write_out_cmd(command, Some(read_data.as_mut_slice()), Duration::from_secs(1))?;

        return Ok(read_data)
    }

    pub fn write(&mut self, dAddr: u32, dSize: u32, mut data: Vec<u8>) -> Result<usize, rusb::Error> {
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
        self.write_out_cmd(command, Some(data.as_mut_slice()), Duration::from_secs(1))
    }

    pub fn exit_xip(&mut self) -> Result<usize, rusb::Error> {
        let cmd_id = 0x6;
        let cmd_size = 0x0;
        let transfer_length = 0x0;

        let args = [0; 16];

        let command = PicobootCommand::new(cmd_id, cmd_size, transfer_length, &args);
        self.write_out_cmd(command, None, Duration::from_secs(1))
    }

    pub fn enter_xip(&mut self) -> Result<usize, rusb::Error> {
        let cmd_id = 0x7;
        let cmd_size = 0x0;
        let transfer_length = 0x0;

        let args = [0; 16];

        let command = PicobootCommand::new(cmd_id, cmd_size, transfer_length, &args);
        self.write_out_cmd(command, None, Duration::from_secs(1))
    }

    pub fn exec(&mut self, dAddr: u32) -> Result<usize, rusb::Error> {
        let cmd_id = 0x8;
        let cmd_size = 0x4;
        let transfer_length = 0x0;

        let mut args = [0; 16];

        let addr_bytes: [u8; 4] = dAddr.to_le_bytes();

        args[0..4].copy_from_slice(&addr_bytes);

        let command = PicobootCommand::new(cmd_id, cmd_size, transfer_length, &args);

        self.write_out_cmd(command, None, Duration::from_secs(1))
    }

    pub fn vectorized_flash(&mut self, dAddr: u32) -> Result<usize, rusb::Error> {
        let cmd_id = 0x9;
        let cmd_size = 0x4;
        let transfer_length = 0x0;

        let mut args = [0; 16];

        let addr_bytes: [u8; 4] = dAddr.to_le_bytes();

        args[0..4].copy_from_slice(&addr_bytes);

        let command = PicobootCommand::new(cmd_id, cmd_size, transfer_length, &args);

        self.write_out_cmd(command, None, Duration::from_secs(1))
    }

    fn is_halted(&mut self, interface: u8) -> Result<bool, rusb::Error> {
        let mut data = [0; 2];
        let mut halted = false;

        self.handle.read_control(
            0x82,
            0x0,
            0,
            interface as u16,
            &mut data,
            Duration::from_secs(1),
        )?;

        if data[0] & 1 == 1 {
            println!("{} was halted!", interface);
            halted = true;
        } else {
            println!("{} was not halted!", interface);
        }

        Ok(halted)
    }

    pub fn interface_reset(&mut self) -> Result<bool, rusb::Error> {
        if self.is_halted(self.device_info.in_addr)? {
            self.handle.clear_halt(self.device_info.in_addr).unwrap();
        }
        if self.is_halted(self.device_info.out_addr)? {
            self.handle.clear_halt(self.device_info.out_addr).unwrap();
        }

        let args = [];

        let transferred = self.handle.write_control(
            0x41,
            0x41,
            0x00,
            self.device_info.iface as u16,
            &args,
            Duration::from_secs(1),
        )?;

        if transferred > 0 {
            return Err(rusb::Error::Other);
        } else {
            Ok(true)
        }
    }

    pub fn get_command_status(&mut self) -> Result<CommandStatus, rusb::Error> {
        let mut response = [0; 16];

        let ret = self.handle.read_control(
            0xC1,
            0x42,
            0x00,
            self.device_info.iface as u16,
            &mut response,
            Duration::from_secs(3),
        )?;

        if ret == 0 {
            println!("Get Command Status failed to populate buffer!");
            return Err(rusb::Error::Other);
        }

        println!("{:?}", response);

        let cmd_status_p = response.as_ptr() as *const CommandStatus;
        let cmd_status = unsafe {*cmd_status_p};

        Ok(cmd_status)

    }


    fn write_out_cmd(&mut self, cmd: PicobootCommand, data: Option<&mut [u8]>, timeout: Duration) -> rusb::Result<usize> {
        if cmd.cmd_id & 0x80 == 0 && cmd.transfer_length != 0 && data.is_none() {
            println!("Data not present for a send command that has a tranfer length!");
            return Err(rusb::Error::Other);
        } else if cmd.cmd_id & 0x80 == 0 && cmd.transfer_length != 0 && data.is_some() && data.as_ref().map(|x| x.len() != cmd.transfer_length.try_into().unwrap()).unwrap() {
            println!("Data is not the same size as the reported transfer_length");
            return Err(rusb::Error::Other);
        }

        let ret = self
            .handle
            .write_bulk(self.device_info.out_addr, cmd.as_ptr(), timeout)?;

        if ret == 0 {
            println!("Failed to send command");
            return Err(rusb::Error::Other);
        }

        if cmd.transfer_length != 0 {
            if cmd.cmd_id & 0x80 != 0 {  
                let ret = self.handle.read_bulk(self.device_info.in_addr, data.unwrap(), timeout)?;
    
                if ret == 0 {
                    println!("Failed to read response for command");
                    return Err(rusb::Error::Other);
                }
            } else {
                let ret = self.handle.write_bulk(self.device_info.out_addr, data.unwrap(), timeout)?;

                if ret == 0 {
                    println!("Failed to send data for command");
                    return Err(rusb::Error::Other);
                }
            }
        }

        

        let mut ack_buf = [0];
        if cmd.cmd_id & 0x80 != 0 {
            let ret = self.handle.write_bulk(self.device_info.out_addr, &mut ack_buf, timeout)?;

            Ok(ret)
        } else {
            let ret = self.handle.read_bulk(self.device_info.in_addr, &mut ack_buf, timeout)?;

            Ok(ret)
        }
    }
}

#[derive(Debug)]
#[repr(C)]
struct PicobootCommand<'a> {
    magic: u32,
    token: u32,
    cmd_id: u8,
    cmd_size: u8,
    reserved: u16,
    transfer_length: u32,
    args: [u8; 16],
    phantom: PhantomData<&'a [u8]>
}

impl<'a> PicobootCommand<'a> {
    fn new(cmd_id: u8, cmd_size: u8, transfer_length: u32, args: &[u8; 16]) -> Self {
        let token = *TOKEN.lock().unwrap();
        *TOKEN.lock().unwrap() = token + 1;
        PicobootCommand {
            magic: 0x431fd10B,
            token: token,
            cmd_id,
            cmd_size,
            reserved: 0x0000,
            transfer_length,
            args: *args,
            phantom: PhantomData,
        }
    }

    fn as_ptr(&'a self) -> &'a [u8] {
        unsafe {
            std::slice::from_raw_parts(
                self as *const _ as *const u8,
                std::mem::size_of::<PicobootCommand>(),
            )
        }
    }
}


#[repr(u8)]
pub enum ExclusivityOption {
    NOT_EXCLUSIVE = 0,
    EXCLUSIVE = 1,
    EXCLUSIVE_AND_EJECT = 2,
}
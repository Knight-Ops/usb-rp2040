use librp2040::*;

fn main() {
    let mut usb_2040 = USB2040::try_find_and_open_2040().unwrap();

    usb_2040.interface_reset().unwrap();
    usb_2040.exclusive_access(ExclusivityOption::EXCLUSIVE_AND_EJECT).unwrap();

    let data = usb_2040.read(0, 0x1000).unwrap();
    println!("{:X?}", data);

    usb_2040.reboot(0, 0, 5000).unwrap();
}

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};

use crossbeam_channel::Receiver;
use tool_core::{Event, LogLevel};
use tool_databus::DataBus;
use windows_sys::Win32::Devices::Communication::{
    CLRDTR, CLRRTS, COMMTIMEOUTS, COMSTAT, DCB, EVENPARITY, EscapeCommFunction, GetCommState,
    NOPARITY, ODDPARITY, ONESTOPBIT, PURGE_RXABORT, PURGE_RXCLEAR, PURGE_TXABORT, PURGE_TXCLEAR,
    PurgeComm, SETDTR, SETRTS, SetCommState, SetCommTimeouts, SetupComm, TWOSTOPBITS,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_GENERIC_WRITE, OPEN_EXISTING,
    ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects,
};

use crate::{
    DataBits, DtrRtsCommand, Parity, SerialConfig, StopBits, TransportResult, serial_rx_event,
    serial_tx_event,
};

pub(crate) struct WakeEvent {
    handle: HANDLE,
}

unsafe impl Send for WakeEvent {}
unsafe impl Sync for WakeEvent {}

impl WakeEvent {
    pub(crate) fn new() -> io::Result<Arc<Self>> {
        // Manual reset event. The worker resets it after draining queued commands.
        let handle = unsafe { CreateEventW(null(), 1, 0, null()) };
        if handle.is_null() {
            Err(last_error())
        } else {
            Ok(Arc::new(Self { handle }))
        }
    }

    pub(crate) fn set(&self) {
        unsafe {
            let _ = SetEvent(self.handle);
        }
    }

    fn reset(&self) {
        unsafe {
            let _ = ResetEvent(self.handle);
        }
    }

    fn raw(&self) -> HANDLE {
        self.handle
    }
}

impl Drop for WakeEvent {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

struct EventHandle {
    handle: HANDLE,
}

impl EventHandle {
    fn new(manual_reset: bool) -> io::Result<Self> {
        let handle = unsafe { CreateEventW(null(), manual_reset as i32, 0, null()) };
        if handle.is_null() {
            Err(last_error())
        } else {
            Ok(Self { handle })
        }
    }

    fn raw(&self) -> HANDLE {
        self.handle
    }

    fn reset(&self) {
        unsafe {
            let _ = ResetEvent(self.handle);
        }
    }
}

impl Drop for EventHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

struct NativeSerialPort {
    handle: HANDLE,
}

unsafe impl Send for NativeSerialPort {}

impl Drop for NativeSerialPort {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

pub(crate) fn spawn_native_serial_worker(
    config: &SerialConfig,
    write_rx: Receiver<Vec<u8>>,
    dtr_rts_rx: Receiver<DtrRtsCommand>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    bus: DataBus,
    source: String,
) -> TransportResult<(JoinHandle<()>, Arc<WakeEvent>)> {
    let port = NativeSerialPort::open(config)?;
    let wake = WakeEvent::new()?;
    let worker_wake = Arc::clone(&wake);
    let join = thread::spawn(move || {
        NativeWorker {
            port,
            write_rx,
            dtr_rts_rx,
            wake: worker_wake,
            stop,
            alive,
            bus,
            source,
        }
        .run();
    });
    Ok((join, wake))
}

struct NativeWorker {
    port: NativeSerialPort,
    write_rx: Receiver<Vec<u8>>,
    dtr_rts_rx: Receiver<DtrRtsCommand>,
    wake: Arc<WakeEvent>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    bus: DataBus,
    source: String,
}

impl NativeWorker {
    fn run(self) {
        match self.run_impl() {
            Ok(()) => {}
            Err(error)
                if !self.stop.load(Ordering::Relaxed)
                    && error.raw_os_error() != Some(ERROR_OPERATION_ABORTED as i32) =>
            {
                self.bus.publish(Event::system_log(
                    LogLevel::Error,
                    "transport.serial",
                    format!("native serial failed on {}: {error}", self.source),
                ));
            }
            Err(_) => {}
        }
        self.alive.store(false, Ordering::Relaxed);
    }

    fn run_impl(&self) -> io::Result<()> {
        let read_event = EventHandle::new(true)?;
        let mut read_overlapped = OVERLAPPED {
            hEvent: read_event.raw(),
            ..Default::default()
        };
        let mut first_byte = [0_u8; 1];
        let mut read_pending = false;

        loop {
            if self.stop.load(Ordering::Acquire) {
                if read_pending {
                    unsafe {
                        let _ = CancelIoEx(self.port.handle, &read_overlapped);
                    }
                }
                return Ok(());
            }

            drain_commands(
                &self.port,
                &self.write_rx,
                &self.dtr_rts_rx,
                &self.bus,
                &self.source,
            )?;
            if self.stop.load(Ordering::Acquire) {
                if read_pending {
                    unsafe {
                        let _ = CancelIoEx(self.port.handle, &read_overlapped);
                    }
                }
                return Ok(());
            }

            if !read_pending {
                read_event.reset();
                match self
                    .port
                    .start_read(&mut first_byte, &mut read_overlapped)?
                {
                    ReadStart::Completed(size) => {
                        if size > 0 {
                            publish_available(
                                &self.port,
                                &self.bus,
                                &self.source,
                                &first_byte[..size],
                            )?;
                        }
                        continue;
                    }
                    ReadStart::Pending => {
                        read_pending = true;
                    }
                }
            }

            let handles = [read_event.raw(), self.wake.raw()];
            let wait = unsafe {
                WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, INFINITE)
            };
            if wait == WAIT_FAILED {
                return Err(last_error());
            }

            if wait == WAIT_OBJECT_0 {
                let mut transferred = 0_u32;
                let ok = unsafe {
                    GetOverlappedResult(self.port.handle, &read_overlapped, &mut transferred, 0)
                };
                read_pending = false;
                if ok == 0 {
                    let error = last_error();
                    if self.stop.load(Ordering::Relaxed)
                        || error.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32)
                    {
                        return Ok(());
                    }
                    return Err(error);
                }
                if transferred > 0 {
                    publish_available(
                        &self.port,
                        &self.bus,
                        &self.source,
                        &first_byte[..(transferred as usize).min(first_byte.len())],
                    )?;
                }
            } else if wait == WAIT_OBJECT_0 + 1 {
                self.wake.reset();
            }
        }
    }
}

fn drain_commands(
    port: &NativeSerialPort,
    write_rx: &Receiver<Vec<u8>>,
    dtr_rts_rx: &Receiver<DtrRtsCommand>,
    bus: &DataBus,
    source: &str,
) -> io::Result<()> {
    while let Ok(cmd) = dtr_rts_rx.try_recv() {
        match cmd {
            DtrRtsCommand::SetDtr(value) => port.set_dtr(value)?,
            DtrRtsCommand::SetRts(value) => port.set_rts(value)?,
        }
    }

    while let Ok(bytes) = write_rx.try_recv() {
        port.write_all(&bytes)?;
        bus.publish(serial_tx_event(source.to_owned(), bytes));
    }
    Ok(())
}

fn publish_available(
    port: &NativeSerialPort,
    bus: &DataBus,
    source: &str,
    first: &[u8],
) -> io::Result<()> {
    let mut data = Vec::with_capacity(first.len() + 4096);
    data.extend_from_slice(first);

    loop {
        let queued = port.bytes_to_read()?;
        if queued == 0 {
            break;
        }
        let mut buffer = vec![0_u8; queued.min(4096)];
        let size = port.read_once(&mut buffer)?;
        if size == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..size]);
        if size < buffer.len() {
            break;
        }
    }

    if !data.is_empty() {
        bus.publish(serial_rx_event(source.to_owned(), data));
    }
    Ok(())
}

enum ReadStart {
    Completed(usize),
    Pending,
}

impl NativeSerialPort {
    fn open(config: &SerialConfig) -> io::Result<Self> {
        let path = windows_port_path(&config.port_name);
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                0,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(last_error());
        }

        let port = Self { handle };
        port.configure(config)?;
        Ok(port)
    }

    fn configure(&self, config: &SerialConfig) -> io::Result<()> {
        cvt(unsafe { SetupComm(self.handle, 64 * 1024, 64 * 1024) })?;
        cvt(unsafe {
            PurgeComm(
                self.handle,
                PURGE_RXABORT | PURGE_RXCLEAR | PURGE_TXABORT | PURGE_TXCLEAR,
            )
        })?;

        let mut dcb = DCB {
            DCBlength: size_of::<DCB>() as u32,
            ..Default::default()
        };
        cvt(unsafe { GetCommState(self.handle, &mut dcb) })?;
        dcb.BaudRate = config.baud_rate;
        dcb.ByteSize = match config.data_bits {
            DataBits::Five => 5,
            DataBits::Six => 6,
            DataBits::Seven => 7,
            DataBits::Eight => 8,
        };
        dcb.Parity = match config.parity {
            Parity::None => NOPARITY,
            Parity::Odd => ODDPARITY,
            Parity::Even => EVENPARITY,
        };
        dcb.StopBits = match config.stop_bits {
            StopBits::One => ONESTOPBIT,
            StopBits::Two => TWOSTOPBITS,
        };
        // DCB bitfield: fBinary=1, fParity follows parity, DTR/RTS enabled, no flow control.
        dcb._bitfield = 1 | ((config.parity != Parity::None) as u32) << 1 | (1 << 4) | (1 << 12);
        cvt(unsafe { SetCommState(self.handle, &dcb) })?;

        let timeouts = COMMTIMEOUTS {
            WriteTotalTimeoutConstant: 5000, // 写超时 5s
            ..Default::default()
        };
        cvt(unsafe { SetCommTimeouts(self.handle, &timeouts) })?;
        Ok(())
    }

    fn start_read(&self, buf: &mut [u8], overlapped: &mut OVERLAPPED) -> io::Result<ReadStart> {
        let mut read = 0_u32;
        let ok = unsafe {
            ReadFile(
                self.handle,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut read,
                overlapped,
            )
        };
        if ok != 0 {
            return Ok(ReadStart::Completed(read as usize));
        }
        let error = unsafe { GetLastError() };
        if error == ERROR_IO_PENDING {
            Ok(ReadStart::Pending)
        } else {
            Err(io::Error::from_raw_os_error(error as i32))
        }
    }

    fn read_once(&self, buf: &mut [u8]) -> io::Result<usize> {
        let event = EventHandle::new(true)?;
        let mut overlapped = OVERLAPPED {
            hEvent: event.raw(),
            ..Default::default()
        };
        let mut read = 0_u32;
        let ok = unsafe {
            ReadFile(
                self.handle,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut read,
                &mut overlapped,
            )
        };
        if ok != 0 {
            return Ok(read as usize);
        }
        wait_overlapped(self.handle, &overlapped, event.raw())
    }

    fn write_all(&self, mut buf: &[u8]) -> io::Result<()> {
        while !buf.is_empty() {
            let written = self.write_once(buf)?;
            if written == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "serial write completed with zero bytes",
                ));
            }
            buf = &buf[written..];
        }
        Ok(())
    }

    fn write_once(&self, buf: &[u8]) -> io::Result<usize> {
        let event = EventHandle::new(true)?;
        let mut overlapped = OVERLAPPED {
            hEvent: event.raw(),
            ..Default::default()
        };
        let mut written = 0_u32;
        let ok = unsafe {
            WriteFile(
                self.handle,
                buf.as_ptr(),
                buf.len() as u32,
                &mut written,
                &mut overlapped,
            )
        };
        if ok != 0 {
            return Ok(written as usize);
        }
        wait_overlapped(self.handle, &overlapped, event.raw())
    }

    fn bytes_to_read(&self) -> io::Result<usize> {
        let mut errors = 0_u32;
        let mut status = COMSTAT::default();
        cvt(unsafe {
            windows_sys::Win32::Devices::Communication::ClearCommError(
                self.handle,
                &mut errors,
                &mut status,
            )
        })?;
        Ok(status.cbInQue as usize)
    }

    fn set_dtr(&self, value: bool) -> io::Result<()> {
        cvt(unsafe { EscapeCommFunction(self.handle, if value { SETDTR } else { CLRDTR }) })
    }

    fn set_rts(&self, value: bool) -> io::Result<()> {
        cvt(unsafe { EscapeCommFunction(self.handle, if value { SETRTS } else { CLRRTS }) })
    }
}

fn wait_overlapped(handle: HANDLE, overlapped: &OVERLAPPED, event: HANDLE) -> io::Result<usize> {
    let error = unsafe { GetLastError() };
    if error != ERROR_IO_PENDING {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    let handles = [event];
    let wait = unsafe { WaitForMultipleObjects(1, handles.as_ptr(), 0, INFINITE) };
    if wait == WAIT_FAILED {
        return Err(last_error());
    }
    let mut transferred = 0_u32;
    cvt(unsafe { GetOverlappedResult(handle, overlapped, &mut transferred, 0) })?;
    Ok(transferred as usize)
}

fn windows_port_path(port_name: &str) -> Vec<u16> {
    let path = if port_name.starts_with(r"\\.\") {
        port_name.to_owned()
    } else {
        format!(r"\\.\{port_name}")
    };
    OsStr::new(&path).encode_wide().chain(Some(0)).collect()
}

fn cvt(ok: i32) -> io::Result<()> {
    if ok == 0 { Err(last_error()) } else { Ok(()) }
}

fn last_error() -> io::Error {
    io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}

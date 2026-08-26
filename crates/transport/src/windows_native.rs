use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};

use crate::RepaintWaker;
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
    INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_GENERIC_WRITE, OPEN_EXISTING,
    ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};

use crate::{
    DataBits, Parity, SerialCommand, SerialConfig, StopBits, TransportResult, serial_rx_event,
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_native_serial_worker(
    config: &SerialConfig,
    command_rx: Receiver<SerialCommand>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    bus: DataBus,
    source: String,
    repaint_waker: Option<Arc<dyn RepaintWaker>>,
) -> TransportResult<(JoinHandle<()>, Arc<WakeEvent>)> {
    let port = NativeSerialPort::open(config)?;
    let wake = WakeEvent::new()?;
    let worker_wake = Arc::clone(&wake);
    let join = thread::spawn(move || {
        NativeWorker {
            port,
            command_rx,
            wake: worker_wake,
            stop,
            alive,
            bus,
            source,
            repaint_waker,
        }
        .run();
    });
    Ok((join, wake))
}

struct NativeWorker {
    port: NativeSerialPort,
    command_rx: Receiver<SerialCommand>,
    wake: Arc<WakeEvent>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    bus: DataBus,
    source: String,
    repaint_waker: Option<Arc<dyn RepaintWaker>>,
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
                    format!(
                        "{} 串口错误：{error}",
                        self.source.trim_start_matches("serial:")
                    ),
                ));
            }
            Err(_) => {}
        }
        // Release 与调用方（send_to/open_serial/reap_dead_ports）的 Acquire load 配对，
        // 确保弱内存模型（ARM/AArch64）上 worker 已死状态对其他线程可见。
        self.alive.store(false, Ordering::Release);
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
                    // MSDN 契约：CancelIoEx 返回 ≠ I/O 完成，必须经 GetOverlappedResult
                    // 确认取消完成方可释放栈上的 OVERLAPPED，否则内核/驱动仍持有其引用。
                    unsafe {
                        let _ = CancelIoEx(self.port.handle, &read_overlapped);
                        let mut transferred = 0_u32;
                        let _ = GetOverlappedResult(
                            self.port.handle,
                            &read_overlapped,
                            &mut transferred,
                            0,
                        );
                    }
                }
                return Ok(());
            }

            drain_commands(
                &self.port,
                &self.command_rx,
                &self.bus,
                &self.source,
                &self.repaint_waker,
            )?;
            if self.stop.load(Ordering::Acquire) {
                if read_pending {
                    unsafe {
                        let _ = CancelIoEx(self.port.handle, &read_overlapped);
                        let mut transferred = 0_u32;
                        let _ = GetOverlappedResult(
                            self.port.handle,
                            &read_overlapped,
                            &mut transferred,
                            0,
                        );
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
                                &self.stop,
                                &self.repaint_waker,
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
                        &self.stop,
                        &self.repaint_waker,
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
    command_rx: &Receiver<SerialCommand>,
    bus: &DataBus,
    source: &str,
    repaint_waker: &Option<Arc<dyn RepaintWaker>>,
) -> io::Result<()> {
    while let Ok(command) = command_rx.try_recv() {
        match command {
            SerialCommand::Write { bytes, completion } => {
                let result = port.write_all(&bytes);
                if let Some(completion) = completion {
                    let _ =
                        completion.send(result.as_ref().map(|_| ()).map_err(ToString::to_string));
                }
                result?;
                bus.publish(serial_tx_event(source.to_owned(), bytes));
                if let Some(w) = repaint_waker {
                    w.wake();
                }
            }
            SerialCommand::SetDtr { value, completion } => {
                let result = port.set_dtr(value);
                if let Some(completion) = completion {
                    let _ =
                        completion.send(result.as_ref().map(|_| ()).map_err(ToString::to_string));
                }
                if let Err(error) = result {
                    bus.publish(Event::system_log(
                        LogLevel::Error,
                        "transport.serial",
                        format!(
                            "{} 设置 DTR 失败：{error}",
                            source.trim_start_matches("serial:")
                        ),
                    ));
                }
            }
            SerialCommand::SetRts { value, completion } => {
                let result = port.set_rts(value);
                if let Some(completion) = completion {
                    let _ =
                        completion.send(result.as_ref().map(|_| ()).map_err(ToString::to_string));
                }
                if let Err(error) = result {
                    bus.publish(Event::system_log(
                        LogLevel::Error,
                        "transport.serial",
                        format!(
                            "{} 设置 RTS 失败：{error}",
                            source.trim_start_matches("serial:")
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn publish_available(
    port: &NativeSerialPort,
    bus: &DataBus,
    source: &str,
    first: &[u8],
    stop: &AtomicBool,
    repaint_waker: &Option<Arc<dyn RepaintWaker>>,
) -> io::Result<()> {
    let mut data = Vec::with_capacity(first.len() + 4096);
    data.extend_from_slice(first);

    // 排空循环加 stop 检查 + 预算，复刻 lib.rs 的 MAX_EXTRA_READS=8 / 5ms 模式，
    // 防止高速持续 RX（USB-CDC cbInQue 始终 >0）下循环无法退出，导致 worker 回不到
    // run_impl 顶部 stop 检查、close_port_blocking 超时。
    const MAX_EXTRA_READS: usize = 8;
    const MAX_EXTRA_READ_DURATION_MS: u64 = 5;
    let started = std::time::Instant::now();
    let mut extra_reads = 0usize;

    loop {
        if stop.load(Ordering::Relaxed)
            || extra_reads >= MAX_EXTRA_READS
            || started.elapsed() > std::time::Duration::from_millis(MAX_EXTRA_READ_DURATION_MS)
        {
            break;
        }
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
        extra_reads += 1;
        if size < buffer.len() {
            break;
        }
    }

    if !data.is_empty() {
        bus.publish(serial_rx_event(source.to_owned(), data));
        if let Some(w) = repaint_waker {
            w.wake();
        }
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
        // A serial driver may leave an overlapped write pending indefinitely
        // (for example after unplug or flow-control trouble). Never let that
        // strand the per-port session and make every later command appear
        // frozen. The cancellation is completed before the stack OVERLAPPED
        // is released.
        wait_overlapped_timeout(self.handle, &overlapped, event.raw(), 5_000)
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
    wait_overlapped_timeout(handle, overlapped, event, INFINITE)
}

fn wait_overlapped_timeout(
    handle: HANDLE,
    overlapped: &OVERLAPPED,
    event: HANDLE,
    timeout_ms: u32,
) -> io::Result<usize> {
    let error = unsafe { GetLastError() };
    if error != ERROR_IO_PENDING {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    let wait = if timeout_ms == INFINITE {
        let handles = [event];
        unsafe { WaitForMultipleObjects(1, handles.as_ptr(), 0, INFINITE) }
    } else {
        unsafe { WaitForSingleObject(event, timeout_ms) }
    };
    if wait == WAIT_FAILED {
        return Err(last_error());
    }
    if wait == WAIT_TIMEOUT {
        unsafe {
            let _ = CancelIoEx(handle, overlapped);
        }
        let mut transferred = 0_u32;
        let completed = unsafe {
            // bWait=TRUE is intentional: CancelIoEx only requests
            // cancellation; this call establishes that the kernel no longer
            // owns the OVERLAPPED before this function returns.
            GetOverlappedResult(handle, overlapped, &mut transferred, 1)
        };
        if completed != 0 {
            return Ok(transferred as usize);
        }
        let cancel_error = last_error();
        if cancel_error.raw_os_error() != Some(ERROR_OPERATION_ABORTED as i32) {
            return Err(cancel_error);
        }
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "serial write timed out",
        ));
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

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
use crossbeam_channel::{Receiver, Sender};
use tool_core::{Event, LogLevel};
use tool_databus::DataBus;
use windows_sys::Win32::Devices::Communication::{
    CLRDTR, CLRRTS, COMMTIMEOUTS, COMSTAT, ClearCommError, DCB, EVENPARITY, EscapeCommFunction,
    GetCommState, NOPARITY, ODDPARITY, ONESTOPBIT, PURGE_RXABORT, PURGE_RXCLEAR, PURGE_TXABORT,
    PURGE_TXCLEAR, PurgeComm, SETDTR, SETRTS, SetCommState, SetCommTimeouts, SetupComm,
    TWOSTOPBITS,
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
        Ok(Arc::new(Self {
            handle: create_event(true)?,
        }))
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
        Ok(Self {
            handle: create_event(manual_reset)?,
        })
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
        // 线程退出（正常返回 **或 run_impl() panic 展开**）时清 alive。原先这里是
        // 函数尾的显式 store：非 panic 路径都能走到，展开路径整体跳过 —— 同一个
        // 僵尸端口缺陷，只是站点不同。Release 与调用方的 Acquire load 配对。
        let _alive_guard = crate::AliveGuard(Arc::clone(&self.alive));
        match self.run_impl() {
            Ok(()) => {}
            Err(error)
                if !self.stop.load(Ordering::Relaxed)
                    && error.raw_os_error() != Some(ERROR_OPERATION_ABORTED as i32) =>
            {
                publish_port_error(&self.bus, &self.source, format_args!("串口错误：{error}"));
            }
            Err(_) => {}
        }
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
                    cancel_pending_read(self.port.handle, &read_overlapped);
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
                    cancel_pending_read(self.port.handle, &read_overlapped);
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

/// MSDN 契约：`CancelIoEx` 返回 ≠ I/O 完成，必须经 `GetOverlappedResult` 确认取消完成
/// 方可释放栈上的 OVERLAPPED，否则内核/驱动仍持有其引用。
fn cancel_pending_read(handle: HANDLE, overlapped: &OVERLAPPED) {
    unsafe {
        let _ = CancelIoEx(handle, overlapped);
        let mut transferred = 0_u32;
        let _ = GetOverlappedResult(handle, overlapped, &mut transferred, 0);
    }
}

/// 把命令的执行结果回填给等待方：成功只报 `Ok(())`，失败只带错误的 `ToString` 文案。
/// 按值接管 `completion`，发送句柄就在本行析构 —— 与原内联写法同一个丢弃点。
fn report_completion(completion: Option<Sender<Result<(), String>>>, result: &io::Result<()>) {
    if let Some(completion) = completion {
        let _ = completion.send(result.as_ref().map(|_| ()).map_err(ToString::to_string));
    }
}

/// 发布一条 `transport.serial` 系统错误日志。端口名用 `trim_start_matches`，它会去掉
/// 连续多个 `serial:` 前缀 —— 与 `extract_port` 的单次 `strip_prefix` 口径并不相同。
fn publish_port_error(bus: &DataBus, source: &str, detail: impl std::fmt::Display) {
    bus.publish(Event::system_log(
        LogLevel::Error,
        "transport.serial",
        format!("{} {detail}", source.trim_start_matches("serial:")),
    ));
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
                report_completion(completion, &result);
                result?;
                bus.publish(serial_tx_event(source.to_owned(), bytes));
                if let Some(w) = repaint_waker {
                    w.wake();
                }
            }
            SerialCommand::SetDtr { value, completion } => {
                let result = port.set_dtr(value);
                report_completion(completion, &result);
                if let Err(error) = result {
                    publish_port_error(bus, source, format_args!("设置 DTR 失败：{error}"));
                }
            }
            SerialCommand::SetRts { value, completion } => {
                let result = port.set_rts(value);
                report_completion(completion, &result);
                if let Err(error) = result {
                    publish_port_error(bus, source, format_args!("设置 RTS 失败：{error}"));
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
        cvt(unsafe { ClearCommError(self.handle, &mut errors, &mut status) })?;
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

/// 创建匿名事件句柄；句柄为空即失败，错误取 `GetLastError()`。
fn create_event(manual_reset: bool) -> io::Result<HANDLE> {
    let handle = unsafe { CreateEventW(null(), manual_reset as i32, 0, null()) };
    if handle.is_null() {
        return Err(last_error());
    }
    Ok(handle)
}

fn last_error() -> io::Error {
    io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}

/// Windows 生产入口的 **guard 站点**测试。
///
/// 本文件整体由 `lib.rs` 的 `#[cfg(windows)] mod windows_native;` 门控，故这里的
/// `windows` 只是把意图写在明面上：非 Windows 的 cfg 路径不编译、也就不需要这条测试。
///
/// 它补上的是 `AliveGuard` 那条共享单测覆盖不到的另一半事实：**那句 guard 确实挂在
/// `NativeWorker::run` 上**。原先"构造不出 NativeSerialPort 所以测不了"的说法不成立
/// —— 该类型只是 `{ handle: HANDLE }`，`WakeEvent::new()` 不需要端口，而 `run_impl`
/// 的第一条语句在 `stop` 已置位时直接 `return Ok(())`：`read_pending == false` 意味着
/// 没有 CancelIoEx / GetOverlappedResult，也没有 WaitForMultipleObjects，全程不碰句柄、
/// 不阻塞、不依赖硬件。
///
/// 仍然测不到的另一半：`run_impl` **内部** panic 展开时 guard 生效 —— 那里没有任何可
/// 注入 panic 的钩子（要真句柄），由共享的 `AliveGuard` 单测（`lib.rs`）代偿。
#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn native_worker_run_clears_alive_flag_when_it_returns() {
        let alive = Arc::new(AtomicBool::new(true));
        let (_writer, command_rx) = crossbeam_channel::bounded::<SerialCommand>(1);
        let worker = NativeWorker {
            // 哨兵句柄：本用例不会有任何 I/O 落到它上面；`Drop` 里的
            // `CloseHandle(INVALID_HANDLE_VALUE)` 只会失败、不会关掉别的真句柄。
            port: NativeSerialPort {
                handle: INVALID_HANDLE_VALUE,
            },
            command_rx,
            wake: WakeEvent::new().expect("创建唤醒事件不需要串口"),
            // 预先置位 ⇒ `run_impl` 第一条语句即返回 Ok(())。
            stop: Arc::new(AtomicBool::new(true)),
            alive: Arc::clone(&alive),
            bus: DataBus::new(),
            source: "serial:GUARD_TEST".to_owned(),
            repaint_waker: None,
        };
        assert!(
            alive.load(Ordering::Acquire),
            "装配前提：worker 还没退出，alive 必须是 true"
        );

        // 直接在当前线程调用：guard 的生效点是 `run` 返回时的栈析构，与它跑在哪个线程
        // 无关 —— 于是这条断言里没有任何等待、超时或调度假设。
        worker.run();

        assert!(
            !alive.load(Ordering::Acquire),
            "`NativeWorker::run`（Windows 上唯一的生产 worker）返回后 alive 必须为 false"
        );
    }
}

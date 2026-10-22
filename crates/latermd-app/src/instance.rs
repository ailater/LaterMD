//! 单实例交接:第二次双击 `.md` 时,把路径送给**已在跑的那个窗口**,
//! 而不是另开一个进程。
//!
//! 背景与必要性的实测依据(别当成过度设计):
//! - egui 0.36 **没有** `Event::OpenFiles`。我在 `egui-0.36.2/src/data/
//!   input/event.rs` 与 `egui-winit-0.36.2` 全 crate grep 过 `OpenFiles`,
//!   零命中。所以「运行中的实例如何得知又有人双击了文件」这个问题
//!   **本框架不给答案**,只能自己搭一条 IPC。
//! - macOS 上已运行的 app 收到文件走 Apple Event(`application:openFile:`),
//!   eframe 不转发那条事件 —— 同一条 IPC 顺便把 mac 的缺口也补上了。
//!
//! ## 协议
//!
//! 锁文件(`<config>/instance.lock`,纯文本)持有 `pid` 与 `port` 两个字段。
//! 写入用 `create_new` 原子创建 —— **谁建成功谁就是主实例**,两个进程
//! 同时启动不会都当主(这是整套机制唯一的竞态点,别改成「先查存在再写」,
//! 那样两个进程都会查到"不存在")。次实例连上主实例的回环端口,把路径
//! 逐帧发过去就退出。
//!
//! 存活判据用 **TCP 连不通**,不用 pid 探活:pid 探活要么引 libc
//! (`kill(pid,0)`),要么读 `/proc` —— 后者 Linux 专有,前者是新增依赖。
//! 而「端口连不上」本身就是主实例已死的**真实**证据:零新增依赖、
//! 三平台同码。`pid` 只留在文件里给用户排查用。
//!
//! 端口是**先占位、后写入**的(锁创建 → 绑定 → 写端口),因此存在一个
//! 「锁存在但内容不完整」的窗口。次实例靠 [`LOCK_WAIT_BUDGET`] 内的
//! 重读跨过它;超预算仍读不到端口就删锁重来(等 2 秒还没写完,主实例
//! 要么已死要么卡死,留着这份锁只会挡住后面的启动)。
//!
//! ## 边界(有意为之,别当 bug 修)
//!
//! 1. **无参数的启动永远开新窗口**,不参与交接。点启动器图标的语义是
//!    「给我一个新窗口」;只有**带着文件路径**的启动才谈得上交接。
//!    连带收益:窗口 A 开着时双击 md → A 里有收件端,md 开成 A 的新标签。
//! 2. **窗口不抢焦点**。eframe 没有跨进程置前的 API,凭空加 Windows
//!    `SetForegroundWindow` / X11 `_NET_ACTIVE_WINDOW` 要三平台三套平台
//!    代码,收益仅是「屏幕上闪一下」。路径确实开进了已有窗口,但用户
//!    可能需要自己 Alt+Tab。
//! 3. **转发失败不重试**。连不上主实例就当自己没看见,正常起进程 ——
//!    强 schemes(防火墙静默丢回环包)下体验是「文件没打开」而非
//!    「点了没反应」。
//!
//! 4. **accept 线程不 join**。它阻塞在 `accept()` 上,退出靠 stop 标志
//!    之后**自己连一下**把自己叫醒(见 [`PrimaryGuard::shutdown`])。

use std::io::{BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::Duration;

/// 锁文件名(落在 `theme::config_dir()` 下,与 settings.json 同级)。
pub const LOCK_FILE_NAME: &str = "instance.lock";

/// 读锁文件时,主实例可能刚 `create_new` 成功但还没写完 `port`。
/// 最多等这么久(每次间隔 [`LOCK_POLL_INTERVAL`])再判「这份锁不可用」。
const LOCK_WAIT_BUDGET: Duration = Duration::from_millis(2_000);

/// 等待期间的轮询间隔。50ms ≈ 两三帧,用户感知不到。
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// 一次认领的最多尝试轮次。第 1 轮 `create_new` 抢锁;失败且锁确实不可用
/// (主实例已死)时删锁,第 2 轮再抢一次。**只两轮**:两轮都失败说明存在
/// 真正的竞态(另一进程正在同一瞬间删锁重建),此时老实当独立实例。
const CLAIM_ATTEMPTS: usize = 2;

/// 连主实例的超时。局域网回环正常是微秒级,500ms 足够宽;再慢就是
/// 防火墙在丢包,快速失败好过让用户对着双击干等。
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

/// 单帧字节上限。防御性上限,不是协议要求:一个正常路径远小于 1 MiB,
/// 超限说明连上的是别人的服务(端口被抢占),早退避免读爆内存。
const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// 主实例侧:启动参数里待打开的路径 + 后续实例的收件端 + 锁守卫。
///
/// 必须整体持有:丢掉 [`PrimaryGuard`] 会立刻删掉锁文件,把单实例能力
/// 悄悄关掉。
#[derive(Debug)]
pub struct Primary {
    /// 启动参数里的路径,上层据此在首帧前打开。
    pub paths: Vec<PathBuf>,
    /// 后续实例转发来的路径。`None` = 本进程不提供交接(见边界 1)。
    incoming: Option<Receiver<Vec<PathBuf>>>,
    guard: Option<PrimaryGuard>,
}

impl Primary {
    /// 取出待转发的路径,每帧由 `App::logic` 归约消费。
    ///
    /// 收件端断连(主实例正在退出)时**把它置为 `None`**:后续每帧不再
    /// `try_recv`,省掉一次无意义的系统调用。
    pub fn take_incoming(&mut self) -> Vec<PathBuf> {
        let Some(receiver) = self.incoming.as_ref() else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        loop {
            match receiver.try_recv() {
                Ok(frames) => paths.extend(frames),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.incoming = None;
                    break;
                }
            }
        }
        paths
    }

    /// 锁文件路径(仅供测试断言「锁在不在」;生产代码不读)。
    #[cfg(test)]
    pub fn lock_path(&self) -> Option<&Path> {
        self.guard
            .as_ref()
            .and_then(|guard| guard.lock_path.as_deref())
    }
}

impl Drop for Primary {
    fn drop(&mut self) {
        if let Some(mut guard) = self.guard.take() {
            guard.shutdown();
        }
    }
}

/// 认领结果:本进程该干什么。
#[derive(Debug)]
pub enum Claim {
    /// 本进程是主实例(或不参与交接),继续正常起窗口。
    Primary(Box<Primary>),
    /// 路径已交给已有实例,本进程应当立即退出。
    HandedOff,
}

/// 锁文件守卫。持有期间锁文件存在,`Drop` 时删除。
#[derive(Debug)]
struct PrimaryGuard {
    lock_path: Option<PathBuf>,
    /// accept 线程的唤醒端口(自己连一下就能把阻塞的 accept 叫醒)。
    wake_port: Option<u16>,
}

impl PrimaryGuard {
    fn shutdown(&mut self) {
        if let Some(path) = self.lock_path.take() {
            // 先删锁再叫醒线程:顺序反了会出现「锁已删但线程还在收」的
            // 窗口 —— 那时新主实例起来了,两个线程抢同一个端口。
            let _ = std::fs::remove_file(&path);
        }
        if let Some(port) = self.wake_port.take() {
            wake(port);
        }
    }
}

/// 尝试认领单实例角色。
///
/// `paths` 是启动参数解析出的待打开文件;`config_dir` 为锁文件目录。
/// 语义:
/// - 抢到锁 → [`Claim::Primary`]**带收件端**(可接收转发)。
/// - 锁在但端口不通(主实例已死)→ 删锁重来,最多 [`CLAIM_ATTEMPTS`] 轮。
/// - 锁在、端口通、且 `paths` 非空 → 转发后 [`Claim::HandedOff`]。
/// - 锁在、端口通、但 `paths` 为空 → [`Claim::Primary`]**不带收件端**
///   (点启动器图标要新窗口,不能被已有实例吞掉)。
/// - 任何一步 IO 失败 → [`Claim::Primary`] 不带收件端(优雅降级成
///   「没有单实例」,而不是拒绝启动)。
pub fn try_claim(paths: Vec<PathBuf>, config_dir: Option<&Path>) -> Claim {
    let Some(dir) = config_dir else {
        return Claim::Primary(Box::new(Primary {
            paths,
            incoming: None,
            guard: None,
        }));
    };
    let lock_path = dir.join(LOCK_FILE_NAME);
    for _ in 0..CLAIM_ATTEMPTS {
        match create_primary_lock(&lock_path) {
            Ok(listener) => {
                // 抢到锁:起 IPC。无路径时也起 —— 开着窗口 A 时双击 md
                // 应当开进 A,这要求 A 有收件端。
                let (sender, receiver) = channel();
                let port = listener
                    .as_ref()
                    .and_then(|listener| listener.local_addr().ok())
                    .map(|addr| addr.port());
                if let Some(listener) = listener {
                    spawn_accept(listener, sender);
                }
                return Claim::Primary(Box::new(Primary {
                    paths,
                    incoming: Some(receiver),
                    guard: Some(PrimaryGuard {
                        lock_path: Some(lock_path),
                        wake_port: port,
                    }),
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if paths.is_empty() {
                    // 没文件要交:老实开自己的窗口,不碰已有实例的锁。
                    return Claim::Primary(Box::new(Primary {
                        paths,
                        incoming: None,
                        guard: None,
                    }));
                }
                match read_live_port(&lock_path) {
                    Some(port) if forward(port, &paths) => return Claim::HandedOff,
                    // 连不上:主实例已死但锁没清(它是被 kill 的)。
                    // 删锁重来 —— 这是 CLAIM_ATTEMPTS 存在的唯一理由。
                    _ => {
                        let _ = std::fs::remove_file(&lock_path);
                    }
                }
            }
            Err(_) => {
                // 建不了锁文件(目录只读/磁盘满):降级成无单实例运行。
                return Claim::Primary(Box::new(Primary {
                    paths,
                    incoming: None,
                    guard: None,
                }));
            }
        }
    }
    Claim::Primary(Box::new(Primary {
        paths,
        incoming: None,
        guard: None,
    }))
}

/// 原子创建锁文件并写入 pid + port。
///
/// 返回绑定好的监听器(绑不上返回 `Ok(None)`,锁仍然占位、端口记 0 —
/// 别的进程读到 0 就当「无交接能力」正常起窗口)。
fn create_primary_lock(lock_path: &Path) -> std::io::Result<Option<TcpListener>> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = match options.open(lock_path) {
        Ok(file) => file,
        // 目录不存在(首次运行,配置目录还没建):先建目录再试一次。
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = lock_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            options.open(lock_path)?
        }
        Err(error) => return Err(error),
    };
    let listener = bind_loopback();
    let port = listener
        .as_ref()
        .and_then(|listener| listener.local_addr().ok())
        .map(|addr| addr.port())
        .unwrap_or(0);
    writeln!(file, "pid={}", std::process::id())?;
    writeln!(file, "port={port}")?;
    // 锁的**存在**是跨进程互斥的全部依据,落盘必须落到底,否则两个主实例。
    file.sync_all()?;
    Ok(listener)
}

/// 绑定回环端口。失败返回 `None`(不 panic:端口被占不该让应用起不来)。
fn bind_loopback() -> Option<TcpListener> {
    TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))).ok()
}

fn spawn_accept(listener: TcpListener, sender: Sender<Vec<PathBuf>>) {
    // spawn 失败时 `sender` 随闭包一起丢 → 收件端立刻断连,上层
    // `take_incoming` 下一帧把它置 None,行为与「没有收件端」一致。
    let _ = thread::Builder::new()
        .name("latermd-instance".into())
        .spawn(move || accept_loop(listener, sender));
}

/// 阻塞式 accept 循环。退出靠 `PrimaryGuard::shutdown` 自己连一下端口
/// (见 [`wake`])—— 比轮询非阻塞 accept 更省电,也没有 20ms 的唤醒噪声。
fn accept_loop(listener: TcpListener, sender: Sender<Vec<PathBuf>>) {
    for stream in listener.incoming() {
        // 连接错误(连接被重置等)是瞬时事件,不当致命:跳过这一条继续收。
        let Ok(stream) = stream else {
            continue;
        };
        // 同步读这一条连接:客户端发完即关,读操作短暂阻塞当前线程。
        // 单线程串行是够的 —— 交接的并发量级是「人手点几下」。
        for path in read_frames(&stream) {
            if sender.send(vec![path]).is_err() {
                // 收件端已被主实例丢弃(正在退出):不再收。
                return;
            }
        }
    }
}

/// 连一下主实例端口,把阻塞中的 `accept()` 叫醒,让 accept 线程自己退出。
fn wake(port: u16) {
    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    if let Ok(stream) = TcpStream::connect_timeout(&address, Duration::from_millis(100)) {
        drop(stream);
    }
}

/// 读一条连接上的全部路径帧。连接协议见 [`encode_frame`]。
fn read_frames(stream: &TcpStream) -> Vec<PathBuf> {
    let mut reader = BufReader::new(stream);
    let mut length_bytes = [0u8; 4];
    let mut paths = Vec::new();
    loop {
        if reader.read_exact(&mut length_bytes).is_err() {
            // 连接关闭或首帧不完整:到此为止。
            return paths;
        }
        let length = u32::from_le_bytes(length_bytes) as usize;
        if length > MAX_FRAME_BYTES {
            return paths;
        }
        let mut payload = vec![0u8; length];
        if reader.read_exact(&mut payload).is_err() {
            return paths;
        }
        paths.push(decode_path(&payload));
    }
}

/// 帧编码:`4 字节小端长度 + 路径原始字节`。
///
/// 长度前缀是必需的,不是过度设计 —— 路径里可能有换行字节(Unix 上合法),
/// 靠行分隔符切帧会把一个文件劈成两个。
fn encode_frame(path: &Path) -> Vec<u8> {
    let payload = path_bytes(path);
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    frame
}

/// 帧解码。字节不是合法平台路径时返回空路径(调用方 `open_path` 会给出
/// 错误提示,而不是让本模块 panic)。
fn decode_path(payload: &[u8]) -> PathBuf {
    path_from_bytes(payload).unwrap_or_default()
}

/// 路径 → 字节。Unix 取 `OsStrExt::as_bytes`(原样,不过 lossy 转换,
/// 生僻文件名不丢字符);Windows 取 UTF-16LE。
#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

#[cfg(windows)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    let mut out = Vec::new();
    for unit in path.as_os_str().encode_wide() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

#[cfg(windows)]
fn path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    // Windows 路径帧按 UTF-16LE 两字节一组；尾部有余数说明帧损坏，不能
    // 猜测补字节，否则会把路径静默解码成另一个文件。
    let (pairs, remainder) = bytes.as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    let units: Vec<u16> = pairs
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    Some(PathBuf::from(std::ffi::OsString::from_wide(&units)))
}

#[cfg(not(any(unix, windows)))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}

#[cfg(not(any(unix, windows)))]
fn path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    std::str::from_utf8(bytes).ok().map(PathBuf::from)
}

/// 解析锁文件里的端口。`None` = 锁不可用(内容缺失/损坏/端口 0/
/// 主实例尚未写完端口且已超预算)。
fn read_live_port(lock_path: &Path) -> Option<u16> {
    let deadline = std::time::Instant::now() + LOCK_WAIT_BUDGET;
    loop {
        if let Some(port) = parse_lock(&std::fs::read_to_string(lock_path).ok()?) {
            return Some(port);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        thread::sleep(LOCK_POLL_INTERVAL);
    }
}

/// 锁文件文本 → 端口。纯函数,便于无头测试钉死格式。
///
/// 格式:`pid=<数字>\nport=<数字>\n`。任一字段缺失/非数字即 `None`。
fn parse_lock(text: &str) -> Option<u16> {
    let mut port = None;
    for line in text.lines() {
        let Some(value) = line.strip_prefix("port=") else {
            continue;
        };
        port = value.trim().parse::<u16>().ok().filter(|port| *port > 0);
    }
    port
}

/// 把路径发给主实例。返回是否成功(**成功即本进程可以退出**)。
fn forward(port: u16, paths: &[PathBuf]) -> bool {
    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) else {
        return false;
    };
    let mut buffer = Vec::new();
    for path in paths {
        buffer.extend_from_slice(&encode_frame(path));
    }
    stream.write_all(&buffer).is_ok() && stream.flush().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时目录:测试内自造,不经环境变量(与项目既有做法一致)。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "latermd-inst-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).expect("建临时目录");
            Self(base)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn md(name: &str) -> PathBuf {
        PathBuf::from(name)
    }

    /// 抢到锁 → 带收件端,且锁文件确实存在。
    #[test]
    fn first_claim_becomes_primary_with_lock() {
        let dir = TempDir::new("primary");
        let claim = try_claim(vec![md("/a.md")], Some(dir.path()));
        assert!(matches!(claim, Claim::Primary(_)), "首次认领应为 Primary");
        assert!(dir.path().join(LOCK_FILE_NAME).exists(), "锁文件应在");
    }

    /// 锁文件里的端口**就是**真在监听的那个端口。
    ///
    /// 判据不是"字段长得对"(那只是文本格式),而是往该端口发一条转发
    /// 能被主实例收到 —— 端口写错、监听起不来、写到别的进程的端口,
    /// 三种错都会让这条红。端口 0 也顺带被否掉(0 不是可连的端口)。
    #[test]
    fn lock_file_port_is_the_live_listener() {
        let dir = TempDir::new("lockfmt");
        let Claim::Primary(mut primary) = try_claim(vec![], Some(dir.path())) else {
            panic!("首次认领应为 Primary");
        };
        let text = std::fs::read_to_string(dir.path().join(LOCK_FILE_NAME)).expect("读锁");
        assert!(
            text.contains(&format!("pid={}", std::process::id())),
            "锁文件应记本进程 pid:{text}"
        );
        let port = parse_lock(&text).expect("端口应可解析");
        assert_ne!(port, 0, "端口不该是 0(那意味着没监听)");

        assert!(
            forward(port, &[md("/probed.md")]),
            "锁文件记的端口必须真能连上并收下转发"
        );
        let mut got = Vec::new();
        for _ in 0..200 {
            got.extend(primary.take_incoming());
            if !got.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got, vec![md("/probed.md")], "转发必须送达同一个收件端");
    }

    /// 端到端语义:「双击第二个 md 开进已有窗口」。
    ///
    /// 走完整三步 —— 真实 `create_primary_lock` 建锁、真实监听、真 socket
    /// 转发、真 channel 收件 —— 不复刻任何一环(此前教训:裸调单函数 +
    /// 手工造状态 = 测不存在的路径)。
    #[test]
    fn second_launch_forwards_paths_into_primary() {
        let dir = TempDir::new("forward");
        let mut primary = match try_claim(vec![], Some(dir.path())) {
            Claim::Primary(primary) => primary,
            Claim::HandedOff => panic!("首次认领不该交接"),
        };

        let handed = try_claim(vec![md("/b.md"), md("/c.md")], Some(dir.path()));
        assert!(
            matches!(handed, Claim::HandedOff),
            "有文件且主实例在跑,应交接:{handed:?}"
        );

        let mut got = Vec::new();
        for _ in 0..200 {
            got.extend(primary.take_incoming());
            if got.len() >= 2 {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got, vec![md("/b.md"), md("/c.md")], "转发的路径要原样送达");
    }

    /// 转发来的路径开成**新标签**,不是覆盖当前标签 —— 多标签语义。
    ///
    /// 走真文件(临时目录落盘)而不是构造假状态:转发来的路径最终要经
    /// `open_path` 读盘,假路径只会验到"读取失败落提示行"。
    #[test]
    fn forwarded_paths_open_as_new_tabs() {
        use crate::state::{Message, State};

        let vault = TempDir::new("tabs");
        let mut notes = Vec::new();
        for name in ["existing.md", "b.md", "c.md"] {
            let path = vault.path().join(name);
            std::fs::write(&path, format!("# {name}\n")).expect("写临时笔记");
            notes.push(path);
        }

        let mut state = State::default();
        state.apply(Message::FileSelected(notes[0].clone()));
        let baseline = state.tabs.tabs.len();
        for note in &notes[1..] {
            state.apply(Message::FileSelected(note.clone()));
        }
        assert_eq!(
            state.tabs.tabs.len(),
            baseline + 2,
            "原标签 + 两个转发来的标签,不该互相覆盖"
        );
        assert_eq!(
            state.tabs.current().document.path.as_deref(),
            Some(notes[2].as_path()),
            "最后转发的那个成为当前标签"
        );
    }

    /// 无路径的第二次启动 → 开自己的窗口(**不**被已有实例吞掉)。
    /// 这是「点启动器图标要新窗口」的那条边界。
    #[test]
    fn pathless_second_launch_keeps_its_own_window() {
        let dir = TempDir::new("pathless");
        let _first = match try_claim(vec![], Some(dir.path())) {
            Claim::Primary(primary) => primary,
            Claim::HandedOff => panic!("首次认领不该交接"),
        };
        match try_claim(vec![], Some(dir.path())) {
            Claim::Primary(primary) => {
                let mut primary = primary;
                assert!(
                    primary.take_incoming().is_empty(),
                    "无文件可交,不该承担收件端"
                );
            }
            Claim::HandedOff => panic!("无路径不该交接(那会让点图标无反应)"),
        }
    }

    /// 僵尸锁(主实例被 kill,端口不通) → 删锁重来,第二次启动成主。
    /// 锁文件由进程退出时删,这里造的是「文件还在但没人听」的残局。
    #[test]
    fn stale_lock_is_reclaimed() {
        let dir = TempDir::new("stale");
        // 端口 1 由内核保留、不会有应用监听 —— 事实上的"死端口"。
        std::fs::write(dir.path().join(LOCK_FILE_NAME), "pid=999999\nport=1\n").expect("造僵尸锁");
        assert!(matches!(
            try_claim(vec![md("/a.md")], Some(dir.path())),
            Claim::Primary(_)
        ));
    }

    /// 损坏的锁文件 → 删锁重开(不是"死端口"那么温和)。
    #[test]
    fn corrupt_lock_is_reclaimed() {
        let dir = TempDir::new("corrupt");
        std::fs::write(dir.path().join(LOCK_FILE_NAME), "这不是锁文件").expect("造坏锁");
        assert!(matches!(
            try_claim(vec![md("/a.md")], Some(dir.path())),
            Claim::Primary(_)
        ));
    }

    /// 配置目录缺席 → 无单实例但照常启动(不 panic、不拒绝启动)。
    #[test]
    fn missing_config_dir_still_starts() {
        let Claim::Primary(mut primary) = try_claim(vec![md("/a.md")], None) else {
            panic!("无目录时不存在主实例");
        };
        assert_eq!(primary.paths, vec![md("/a.md")], "路径仍要交给上层打开");
        assert!(primary.take_incoming().is_empty());
    }

    /// 持锁者退出 → 锁文件消失,下一个进程能正常抢到锁。
    /// `Drop` 删锁这条不测,后面所有单实例测试都会莫名变成"第二次启动
    /// 交接给上一个测试留下的实例"。
    #[test]
    fn dropping_primary_releases_the_lock() {
        let dir = TempDir::new("release");
        {
            let Claim::Primary(primary) = try_claim(vec![], Some(dir.path())) else {
                panic!("首次认领应为 Primary");
            };
            assert!(primary.lock_path().is_some());
        }
        assert!(
            !dir.path().join(LOCK_FILE_NAME).exists(),
            "守卫 Drop 后锁文件应消失"
        );
        // 且能立刻再抢到(不是"文件没了但端口还占着"的状态)
        assert!(matches!(
            try_claim(vec![], Some(dir.path())),
            Claim::Primary(_)
        ));
    }

    /// 帧编解码往返,含空格与非 ASCII 路径。
    #[test]
    fn frame_round_trip_preserves_path() {
        for path in ["/a.md", "/我的 笔记/x.md", "/a b/c.md"] {
            let encoded = encode_frame(Path::new(path));
            assert_eq!(&encoded[..4], &(path.len() as u32).to_le_bytes()[..]);
            assert_eq!(decode_path(&encoded[4..]), PathBuf::from(path));
        }
    }

    /// 帧长度前缀的必要性:路径里有换行字节时靠行分隔会切错帧。
    #[test]
    fn frame_survives_newline_in_path() {
        let path = PathBuf::from("/we\nird/a.md");
        let encoded = encode_frame(&path);
        assert_eq!(decode_path(&encoded[4..]), path);
    }

    /// 锁文件解析:正常、缺字段、非数字、端口 0、字段顺序颠倒。
    #[test]
    fn parse_lock_covers_formats() {
        assert_eq!(parse_lock("pid=1\nport=8731\n"), Some(8731));
        assert_eq!(parse_lock("port=8731\npid=1\n"), Some(8731), "顺序不敏感");
        assert_eq!(parse_lock("pid=1\n"), None, "缺端口");
        assert_eq!(parse_lock("port=abc\n"), None, "非数字");
        assert_eq!(parse_lock("port=0\n"), None, "0 = 没监听,不是有效端口");
        assert_eq!(parse_lock(""), None);
    }
}

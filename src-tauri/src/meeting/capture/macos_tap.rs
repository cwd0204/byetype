//! macOS 系统音频：Core Audio 进程 tap（macOS 14.2+）。
//!
//! 流程：`CATapDescription`（全局混音、排除自身进程、mono）→ `AudioHardwareCreateProcessTap`
//! → 建一个私有聚合设备把 tap 挂进去 → `AudioDeviceCreateIOProcID` 读输入 → 送进采集线程。
//!
//! - 聚合设备 / IOProc / 属性读取走 coreaudio-sys（cpal 已依赖，符号齐全）
//! - `AudioHardwareCreateProcessTap` / `DestroyProcessTap` 是 14.2+ 新符号，用 dlsym 运行时解析，
//!   否则 minimumSystemVersion 10.15 的包在老系统上 dyld 直接拒绝启动
//! - `CATapDescription` 是 ObjC 类，走 objc crate 的 msg_send
//! - 权限：TCC「仅系统音频录制」，首次创建 tap 时弹窗；Info.plist 需要 NSAudioCaptureUsageDescription

use std::ffi::{c_void, CStr};
use std::mem;
use std::ptr;
use std::sync::mpsc;
use std::time::Duration;

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use coreaudio_sys::{
    kAudioDevicePropertyDeviceUID, kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyTranslatePIDToProcessObject,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
    kAudioTapPropertyFormat, AudioBufferList, AudioDeviceCreateIOProcID,
    AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop,
    AudioHardwareCreateAggregateDevice, AudioHardwareDestroyAggregateDevice,
    AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
    AudioStreamBasicDescription, AudioTimeStamp, OSStatus,
};
use objc::runtime::{Class, Object, YES};
use objc::{class, msg_send, sel, sel_impl};

use super::{CaptureBackend, FrameSender, PcmChunk, Track};

type CreateTapFn = unsafe extern "C" fn(*mut Object, *mut AudioObjectID) -> OSStatus;
type DestroyTapFn = unsafe extern "C" fn(AudioObjectID) -> OSStatus;

const TAP_NAME: &str = "ByeType Meeting Tap";

fn load_symbol<T: Copy>(name: &CStr) -> Option<T> {
    // SAFETY: dlsym 只做符号查找；返回的指针按调用方声明的函数签名解释，
    // 两个 Tap 函数的签名来自 Apple 头文件 AudioHardwareTapping.h。
    let ptr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    if ptr.is_null() {
        return None;
    }
    assert_eq!(mem::size_of::<T>(), mem::size_of::<*mut c_void>());
    Some(unsafe { mem::transmute_copy::<*mut c_void, T>(&ptr) })
}

fn create_tap_fn() -> Option<CreateTapFn> {
    load_symbol(c"AudioHardwareCreateProcessTap")
}

fn destroy_tap_fn() -> Option<DestroyTapFn> {
    load_symbol(c"AudioHardwareDestroyProcessTap")
}

/// 是否支持系统音频录制；Err 是原因文案。
pub fn support_status() -> Result<(), String> {
    if create_tap_fn().is_none() || destroy_tap_fn().is_none() {
        return Err("需要 macOS 14.2 或更新版本（系统缺少 Core Audio 进程 tap 接口）".to_string());
    }
    if Class::get("CATapDescription").is_none() {
        return Err("需要 macOS 14.2 或更新版本（缺少 CATapDescription）".to_string());
    }
    Ok(())
}

fn global_address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn status_err(what: &str, status: OSStatus) -> String {
    format!("{} 失败（OSStatus {}）", what, status)
}

/// 本进程对应的 Core Audio 进程对象，用于把自己排除在全局 tap 之外。
fn own_process_object() -> Option<AudioObjectID> {
    let pid: libc::pid_t = unsafe { libc::getpid() };
    let address = global_address(kAudioHardwarePropertyTranslatePIDToProcessObject);
    let mut object: AudioObjectID = 0;
    let mut size = mem::size_of::<AudioObjectID>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject,
            &address,
            mem::size_of::<libc::pid_t>() as u32,
            &pid as *const libc::pid_t as *const c_void,
            &mut size,
            &mut object as *mut AudioObjectID as *mut c_void,
        )
    };
    if status == 0 && object != 0 {
        Some(object)
    } else {
        None
    }
}

/// 默认输出设备的 UID；聚合设备把它作为主子设备，tap 才有稳定的时钟来源。
fn default_output_uid() -> Option<String> {
    let address = global_address(kAudioHardwarePropertyDefaultOutputDevice);
    let mut device: AudioObjectID = 0;
    let mut size = mem::size_of::<AudioObjectID>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject,
            &address,
            0,
            ptr::null(),
            &mut size,
            &mut device as *mut AudioObjectID as *mut c_void,
        )
    };
    if status != 0 || device == 0 {
        return None;
    }
    let address = global_address(kAudioDevicePropertyDeviceUID);
    let mut uid_ref: coreaudio_sys::CFStringRef = ptr::null();
    let mut size = mem::size_of::<coreaudio_sys::CFStringRef>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            &address,
            0,
            ptr::null(),
            &mut size,
            &mut uid_ref as *mut coreaudio_sys::CFStringRef as *mut c_void,
        )
    };
    if status != 0 || uid_ref.is_null() {
        return None;
    }
    // SAFETY: 属性读取返回 +1 引用的 CFString，wrap_under_create_rule 接管释放。
    let uid = unsafe {
        CFString::wrap_under_create_rule(uid_ref as core_foundation::string::CFStringRef)
    };
    Some(uid.to_string())
}

/// 读取 tap 的音频格式（采样率、声道、是否 float / 非交织）。
fn tap_format(tap: AudioObjectID) -> Result<AudioStreamBasicDescription, String> {
    let address = global_address(kAudioTapPropertyFormat);
    let mut asbd: AudioStreamBasicDescription = unsafe { mem::zeroed() };
    let mut size = mem::size_of::<AudioStreamBasicDescription>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            tap,
            &address,
            0,
            ptr::null(),
            &mut size,
            &mut asbd as *mut AudioStreamBasicDescription as *mut c_void,
        )
    };
    if status != 0 {
        return Err(status_err("读取 tap 音频格式", status));
    }
    Ok(asbd)
}

struct TapDescription {
    object: *mut Object,
    uuid: String,
}

impl Drop for TapDescription {
    fn drop(&mut self) {
        if !self.object.is_null() {
            let _: () = unsafe { msg_send![self.object, release] };
        }
    }
}

/// 全局 mono 混音 tap，排除自身进程（否则会把自己播放的提示音也录进去）。
fn make_tap_description(exclude: &[AudioObjectID]) -> Result<TapDescription, String> {
    let cls = Class::get("CATapDescription")
        .ok_or_else(|| "CATapDescription 不可用（需要 macOS 14.2+）".to_string())?;
    unsafe {
        let numbers: Vec<*mut Object> = exclude
            .iter()
            .map(|&id| {
                let number: *mut Object = msg_send![class!(NSNumber), numberWithUnsignedInt: id];
                number
            })
            .collect();
        let array: *mut Object =
            msg_send![class!(NSArray), arrayWithObjects: numbers.as_ptr() count: numbers.len()];

        let desc: *mut Object = msg_send![cls, alloc];
        let desc: *mut Object = msg_send![desc, initMonoGlobalTapButExcludeProcesses: array];
        if desc.is_null() {
            return Err("初始化 CATapDescription 失败".to_string());
        }
        let name: *mut Object =
            msg_send![class!(NSString), stringWithUTF8String: c"ByeType Meeting Tap".as_ptr()];
        let _: () = msg_send![desc, setName: name];
        let _: () = msg_send![desc, setPrivate: YES];
        // CATapMuteBehavior: 0 = CATapUnmuted，用户照常能听到会议声音
        let _: () = msg_send![desc, setMuteBehavior: 0isize];

        let uuid_obj: *mut Object = msg_send![desc, UUID];
        let uuid_str: *mut Object = msg_send![uuid_obj, UUIDString];
        let utf8: *const libc::c_char = msg_send![uuid_str, UTF8String];
        let uuid = if utf8.is_null() {
            String::new()
        } else {
            CStr::from_ptr(utf8).to_string_lossy().to_string()
        };
        if uuid.is_empty() {
            let _: () = msg_send![desc, release];
            return Err("读取 tap UUID 失败".to_string());
        }
        Ok(TapDescription { object: desc, uuid })
    }
}

fn cf_pair(key: &str, value: CFType) -> (CFString, CFType) {
    (CFString::new(key), value)
}

/// 私有聚合设备：主子设备 = 默认输出设备，tap 列表 = 我们的 tap（开漂移补偿、自动启动）。
fn create_aggregate(tap_uuid: &str) -> Result<AudioObjectID, String> {
    let sub_tap = CFDictionary::from_CFType_pairs(&[
        cf_pair("uid", CFString::new(tap_uuid).as_CFType()),
        cf_pair("drift", CFBoolean::true_value().as_CFType()),
    ]);
    let taps = CFArray::from_CFTypes(&[sub_tap.as_CFType()]);

    let mut pairs: Vec<(CFString, CFType)> = vec![
        cf_pair(
            "uid",
            CFString::new(&format!("com.byetype.meeting-tap.{}", tap_uuid)).as_CFType(),
        ),
        cf_pair("name", CFString::new(TAP_NAME).as_CFType()),
        cf_pair("private", CFBoolean::true_value().as_CFType()),
        cf_pair("tapautostart", CFBoolean::true_value().as_CFType()),
        cf_pair("taps", taps.as_CFType()),
    ];
    if let Some(output_uid) = default_output_uid() {
        let sub_device = CFDictionary::from_CFType_pairs(&[cf_pair(
            "uid",
            CFString::new(&output_uid).as_CFType(),
        )]);
        pairs.push(cf_pair("master", CFString::new(&output_uid).as_CFType()));
        pairs.push(cf_pair(
            "subdevices",
            CFArray::from_CFTypes(&[sub_device.as_CFType()]).as_CFType(),
        ));
    }
    let description = CFDictionary::from_CFType_pairs(&pairs);

    let mut device: AudioObjectID = 0;
    let status = unsafe {
        AudioHardwareCreateAggregateDevice(
            description.as_concrete_TypeRef() as *const c_void as coreaudio_sys::CFDictionaryRef,
            &mut device,
        )
    };
    if status != 0 || device == 0 {
        return Err(status_err("创建聚合设备", status));
    }
    Ok(device)
}

struct TapContext {
    tx: FrameSender,
    sample_rate: u32,
    channels: u16,
    is_float: bool,
    non_interleaved: bool,
    bits: u32,
}

unsafe extern "C" fn io_proc(
    _device: AudioObjectID,
    _now: *const AudioTimeStamp,
    input: *const AudioBufferList,
    _input_time: *const AudioTimeStamp,
    _output: *mut AudioBufferList,
    _output_time: *const AudioTimeStamp,
    client: *mut c_void,
) -> OSStatus {
    if input.is_null() || client.is_null() {
        return 0;
    }
    let ctx = &*(client as *const TapContext);
    let list = &*input;
    let count = list.mNumberBuffers as usize;
    if count == 0 {
        return 0;
    }
    // mBuffers 声明为长度 1 的数组，实际按 mNumberBuffers 连续排布
    let buffers = std::slice::from_raw_parts(list.mBuffers.as_ptr(), count);

    let mut samples: Vec<f32> = Vec::new();
    let channels: u16;
    if ctx.non_interleaved {
        // 每个 buffer 一条声道：这里只取第一条（tap 本身已是 mono 混音）
        let first = &buffers[0];
        samples = decode_buffer(
            first.mData,
            first.mDataByteSize as usize,
            ctx.is_float,
            ctx.bits,
        );
        channels = 1;
    } else {
        for buffer in buffers {
            samples.extend(decode_buffer(
                buffer.mData,
                buffer.mDataByteSize as usize,
                ctx.is_float,
                ctx.bits,
            ));
        }
        channels = ctx.channels.max(1);
    }
    if !samples.is_empty() {
        let _ = ctx.tx.send((
            Track::System,
            PcmChunk {
                samples,
                sample_rate: ctx.sample_rate,
                channels,
            },
        ));
    }
    0
}

unsafe fn decode_buffer(data: *mut c_void, bytes: usize, is_float: bool, bits: u32) -> Vec<f32> {
    if data.is_null() || bytes == 0 {
        return Vec::new();
    }
    if is_float && bits == 32 {
        let count = bytes / 4;
        std::slice::from_raw_parts(data as *const f32, count).to_vec()
    } else if !is_float && bits == 16 {
        let count = bytes / 2;
        std::slice::from_raw_parts(data as *const i16, count)
            .iter()
            .map(|&s| s as f32 / 32768.0)
            .collect()
    } else if !is_float && bits == 32 {
        let count = bytes / 4;
        std::slice::from_raw_parts(data as *const i32, count)
            .iter()
            .map(|&s| s as f32 / 2_147_483_648.0)
            .collect()
    } else {
        Vec::new()
    }
}

pub struct ProcessTapBackend {
    tap: Option<AudioObjectID>,
    aggregate: Option<AudioObjectID>,
    io_proc_id: AudioDeviceIOProcID,
    ctx: Option<Box<TapContext>>,
    destroy_tap: Option<DestroyTapFn>,
}

impl ProcessTapBackend {
    pub fn new() -> Self {
        Self {
            tap: None,
            aggregate: None,
            io_proc_id: None,
            ctx: None,
            destroy_tap: None,
        }
    }
}

impl Default for ProcessTapBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureBackend for ProcessTapBackend {
    fn start(&mut self, tx: FrameSender) -> Result<(), String> {
        let create_tap = create_tap_fn().ok_or_else(|| {
            "需要 macOS 14.2 或更新版本（缺少 AudioHardwareCreateProcessTap）".to_string()
        })?;
        let destroy_tap = destroy_tap_fn().ok_or_else(|| {
            "需要 macOS 14.2 或更新版本（缺少 AudioHardwareDestroyProcessTap）".to_string()
        })?;

        let exclude: Vec<AudioObjectID> = own_process_object().into_iter().collect();
        let description = make_tap_description(&exclude)?;

        let mut tap: AudioObjectID = 0;
        let status = unsafe { create_tap(description.object, &mut tap) };
        if status != 0 || tap == 0 {
            return Err(format!(
                "{}；如果是首次使用，请在「系统设置 → 隐私与安全性 → 屏幕与系统音频录制」里允许 ByeType 录制系统音频",
                status_err("创建系统音频 tap", status)
            ));
        }
        self.tap = Some(tap);
        self.destroy_tap = Some(destroy_tap);

        let asbd = match tap_format(tap) {
            Ok(asbd) => asbd,
            Err(error) => {
                self.stop();
                return Err(error);
            }
        };
        let aggregate = match create_aggregate(&description.uuid) {
            Ok(device) => device,
            Err(error) => {
                self.stop();
                return Err(error);
            }
        };
        self.aggregate = Some(aggregate);

        let ctx = Box::new(TapContext {
            tx,
            sample_rate: asbd.mSampleRate.max(1.0) as u32,
            channels: asbd.mChannelsPerFrame.max(1) as u16,
            is_float: asbd.mFormatFlags & kAudioFormatFlagIsFloat != 0,
            non_interleaved: asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0,
            bits: asbd.mBitsPerChannel,
        });
        let ctx_ptr: *const TapContext = &*ctx;
        self.ctx = Some(ctx);

        let mut proc_id: AudioDeviceIOProcID = None;
        let status = unsafe {
            AudioDeviceCreateIOProcID(
                aggregate,
                Some(io_proc),
                ctx_ptr as *mut c_void,
                &mut proc_id,
            )
        };
        if status != 0 || proc_id.is_none() {
            self.stop();
            return Err(status_err("注册音频回调", status));
        }
        self.io_proc_id = proc_id;

        let status = unsafe { AudioDeviceStart(aggregate, proc_id) };
        if status != 0 {
            self.stop();
            return Err(status_err("启动聚合设备", status));
        }
        Ok(())
    }

    fn stop(&mut self) {
        unsafe {
            if let Some(aggregate) = self.aggregate.take() {
                if let Some(proc_id) = self.io_proc_id.take() {
                    let _ = AudioDeviceStop(aggregate, Some(proc_id));
                    let _ = AudioDeviceDestroyIOProcID(aggregate, Some(proc_id));
                }
                let _ = AudioHardwareDestroyAggregateDevice(aggregate);
            }
            if let (Some(tap), Some(destroy)) = (self.tap.take(), self.destroy_tap) {
                let _ = destroy(tap);
            }
        }
        // 回调已停止，现在释放上下文才安全
        self.ctx = None;
    }
}

impl Drop for ProcessTapBackend {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 自检：建一次 tap 再拆掉，用来主动触发 TCC 授权弹窗与验证可用性。
pub fn probe(duration_ms: u64) -> Result<(), String> {
    support_status()?;
    let (tx, rx) = mpsc::channel::<(Track, PcmChunk)>();
    let mut backend = ProcessTapBackend::new();
    backend.start(tx)?;
    std::thread::sleep(Duration::from_millis(duration_ms));
    backend.stop();
    drop(rx);
    Ok(())
}

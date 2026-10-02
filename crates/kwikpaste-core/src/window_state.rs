//! 窗口位置与尺寸的存档文件（`state/` 目录下），只做纯文件读写，不碰窗口。
//!
//! - `window-state.gpui.json`：原生版自己的存档，逻辑像素，按窗口 label 存
//!   `{x, y, width, height, scale}`，`scale` 是保存时所在显示器的缩放。
//! - `window-state.json`：1.x 的存档，`{label: {x, y, width, height}}`，物理像素
//!   （macOS 上是 point × backingScaleFactor）。原生版只在没有自己的存档时读一次并换算，
//!   永远不写，留给回装的 1.x 使用。
//!
//! 换算要知道各显示器的物理范围和缩放，这些由宿主（平台层）提供，见 [`legacy_to_logical`]。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, RwLock};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::paths::CorePaths;

/// 原生版的窗口存档文件名。
const NATIVE_STATE_FILENAME: &str = "window-state.gpui.json";
/// 1.x 的窗口存档文件名，只读。
const LEGACY_STATE_FILENAME: &str = "window-state.json";
/// 坐标绝对值的合理上限。从未显示过的窗口取到的几何是 −1431655800 这类垃圾值，不能存。
const MAX_COORDINATE: f64 = 1_000_000.0;

/// 原生版保存的窗口几何，逻辑像素。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// 保存时窗口所在显示器的缩放（物理像素 / 逻辑像素）。
    pub scale: f64,
}

/// 1.x 保存的窗口几何：`outer_position` 与 `inner_size`，物理像素。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyWindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// 一块显示器在 1.x 坐标系里的物理范围与缩放，由平台层提供。
///
/// Windows 是虚拟桌面的物理像素；macOS 是 tao 的「物理」值，即 NSScreen 的 frame
/// 乘以该屏的 backingScaleFactor（不同缩放的屏之间可能重叠或留缝，按 1.x 当时的算法给即可）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LegacyMonitor {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
}

impl LegacyMonitor {
    fn contains(&self, x: i32, y: i32) -> bool {
        let right = i64::from(self.x) + i64::from(self.width);
        let bottom = i64::from(self.y) + i64::from(self.height);
        x >= self.x && i64::from(x) < right && y >= self.y && i64::from(y) < bottom
    }
}

impl LegacyWindowState {
    /// 按给定缩放换算成逻辑像素。
    pub fn to_logical(&self, scale: f64) -> WindowGeometry {
        WindowGeometry {
            x: f64::from(self.x) / scale,
            y: f64::from(self.y) / scale,
            width: f64::from(self.width) / scale,
            height: f64::from(self.height) / scale,
            scale,
        }
    }
}

/// 找到左上角所在的显示器，按它的缩放把 1.x 的物理像素换算成逻辑像素。
/// 左上角不在任何显示器上（显示器已拔掉）或缩放不合法时返回 `None`，宿主按默认位置摆放。
pub fn legacy_to_logical(
    state: &LegacyWindowState,
    monitors: &[LegacyMonitor],
) -> Option<WindowGeometry> {
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.contains(state.x, state.y))?;
    if !(monitor.scale.is_finite() && monitor.scale > 0.0) {
        return None;
    }

    Some(state.to_logical(monitor.scale))
}

/// 窗口存档。原生存档常驻内存，每次保存整份写回；1.x 存档只在原生存档不存在时载入，等宿主换算。
pub struct WindowStateStore {
    dir: RwLock<PathBuf>,
    states: Mutex<HashMap<String, WindowGeometry>>,
    /// 原生存档不存在时读到的 1.x 存档；[`Self::migrate_legacy`] 换算后清空。
    legacy: Mutex<Option<HashMap<String, LegacyWindowState>>>,
}

impl WindowStateStore {
    pub fn new(paths: &CorePaths) -> Result<Self> {
        let dir = state_dir(paths)?;
        let (states, legacy) = load(&dir);
        log::info!("window state store ready at {dir:?}");

        Ok(Self {
            dir: RwLock::new(dir),
            states: Mutex::new(states),
            legacy: Mutex::new(legacy),
        })
    }

    /// 原生存档里某个窗口的几何。
    pub fn get(&self, label: &str) -> Option<WindowGeometry> {
        self.states().get(label).copied()
    }

    /// 保存一个窗口的几何并整份写回原生存档。只在窗口显示过之后调用；
    /// 非有限值、非正尺寸和明显越界的坐标直接拒绝，不落盘。
    pub fn save(&self, label: &str, geometry: WindowGeometry) -> Result<()> {
        validate(&geometry)?;

        let mut states = self.states();
        states.insert(label.to_owned(), geometry);
        write_states(&self.native_path(), &states)
    }

    /// 是否还有待换算的 1.x 存档（原生存档不存在时才会有）。
    pub fn has_pending_legacy(&self) -> bool {
        self.legacy().is_some()
    }

    /// 首次启动时把 1.x 存档逐个窗口换算成逻辑像素并写成原生存档，之后不再读 1.x 存档。
    ///
    /// `convert` 通常是用当前显示器列表调用 [`legacy_to_logical`]；返回 `None` 的窗口不迁移。
    /// 即使一个窗口都没迁移也会写出原生存档，标记迁移已完成。没有待换算的存档时什么都不做，返回 0。
    pub fn migrate_legacy<F>(&self, mut convert: F) -> Result<usize>
    where
        F: FnMut(&str, &LegacyWindowState) -> Option<WindowGeometry>,
    {
        let mut legacy = self.legacy();
        let Some(pending) = legacy.as_ref() else {
            return Ok(0);
        };

        let mut states = self.states();
        let mut migrated = 0;
        for (label, state) in pending {
            if states.contains_key(label) {
                continue;
            }
            let Some(geometry) = convert(label, state) else {
                continue;
            };
            if validate(&geometry).is_err() {
                continue;
            }
            states.insert(label.clone(), geometry);
            migrated += 1;
        }

        write_states(&self.native_path(), &states)?;
        *legacy = None;
        log::info!("migrated {migrated} window state(s) from {LEGACY_STATE_FILENAME}");
        Ok(migrated)
    }

    /// 数据目录热切换后重新绑定存档目录，并重新读取新目录里的存档。
    pub fn rebase(&self, paths: &CorePaths) -> Result<()> {
        let dir = state_dir(paths)?;
        let (states, legacy) = load(&dir);

        *self.dir.write().expect("window state dir poisoned") = dir;
        *self.states() = states;
        *self.legacy() = legacy;
        Ok(())
    }

    fn native_path(&self) -> PathBuf {
        self.dir
            .read()
            .expect("window state dir poisoned")
            .join(NATIVE_STATE_FILENAME)
    }

    fn states(&self) -> MutexGuard<'_, HashMap<String, WindowGeometry>> {
        self.states
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn legacy(&self) -> MutexGuard<'_, Option<HashMap<String, LegacyWindowState>>> {
        self.legacy
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn state_dir(paths: &CorePaths) -> Result<PathBuf> {
    let dir = paths.state_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("failed to create dir at {dir:?}"))?;
    Ok(dir)
}

/// 读原生存档；它不存在时再读 1.x 存档作为待换算数据。读不懂的文件按空处理，与 1.x 一致。
#[allow(clippy::type_complexity)]
fn load(
    dir: &Path,
) -> (
    HashMap<String, WindowGeometry>,
    Option<HashMap<String, LegacyWindowState>>,
) {
    let native = dir.join(NATIVE_STATE_FILENAME);
    if native.exists() {
        return (read_states(&native), None);
    }

    let legacy = dir.join(LEGACY_STATE_FILENAME);
    let pending = legacy.exists().then(|| read_states(&legacy));
    (HashMap::new(), pending)
}

fn read_states<T: serde::de::DeserializeOwned>(path: &Path) -> HashMap<String, T> {
    match fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|err| {
            log::warn!("failed to parse window state at {path:?}, using defaults: {err}");
            HashMap::new()
        }),
        Err(err) => {
            log::warn!("failed to read window state at {path:?}, using defaults: {err}");
            HashMap::new()
        }
    }
}

fn write_states(path: &Path, states: &HashMap<String, WindowGeometry>) -> Result<()> {
    let json = serde_json::to_string_pretty(states).context("failed to serialize window states")?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).with_context(|| format!("failed to write {tmp:?}"))?;
    fs::rename(&tmp, path).with_context(|| format!("failed to promote {tmp:?} to {path:?}"))?;
    Ok(())
}

fn validate(geometry: &WindowGeometry) -> Result<()> {
    let WindowGeometry {
        x,
        y,
        width,
        height,
        scale,
    } = *geometry;
    let finite = [x, y, width, height, scale]
        .iter()
        .all(|value| value.is_finite());
    let in_range = x.abs() <= MAX_COORDINATE
        && y.abs() <= MAX_COORDINATE
        && width > 0.0
        && height > 0.0
        && width <= MAX_COORDINATE
        && height <= MAX_COORDINATE
        && scale > 0.0;
    if finite && in_range {
        return Ok(());
    }

    Err(AppError::Other(anyhow::anyhow!(
        "invalid window geometry: {geometry:?}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::AppEnv;

    fn paths(temp: &tempfile::TempDir) -> CorePaths {
        let local = temp.path().join("local");
        CorePaths::new(AppEnv::Dev, local.clone(), local.join("logs"), None)
    }

    fn write_legacy(paths: &CorePaths, content: &str) -> PathBuf {
        let dir = paths.state_dir().unwrap();
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(LEGACY_STATE_FILENAME);
        fs::write(&path, content).unwrap();
        path
    }

    fn geometry(x: f64, y: f64) -> WindowGeometry {
        WindowGeometry {
            x,
            y,
            width: 400.0,
            height: 600.0,
            scale: 1.5,
        }
    }

    #[test]
    fn save_round_trips_through_native_file() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(&temp);
        let store = WindowStateStore::new(&paths).unwrap();

        store.save("main", geometry(10.5, -20.0)).unwrap();

        let reopened = WindowStateStore::new(&paths).unwrap();
        assert_eq!(reopened.get("main"), Some(geometry(10.5, -20.0)));
        assert_eq!(reopened.get("preference"), None);
        assert!(paths
            .state_dir()
            .unwrap()
            .join(NATIVE_STATE_FILENAME)
            .is_file());
    }

    #[test]
    fn garbage_geometry_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let store = WindowStateStore::new(&paths(&temp)).unwrap();

        assert!(store
            .save("main", geometry(-1_431_655_800.0, -1_431_655_800.0))
            .is_err());
        assert!(store.save("main", geometry(f64::NAN, 0.0)).is_err());
        assert!(store
            .save(
                "main",
                WindowGeometry {
                    width: 0.0,
                    ..geometry(0.0, 0.0)
                }
            )
            .is_err());
        assert_eq!(store.get("main"), None);
    }

    /// 1.x 写下的存档原样读入，按所在显示器的缩放换算；换算后旧文件一个字节都不变。
    #[test]
    fn legacy_file_is_converted_once_and_never_written() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(&temp);
        let legacy_content = "{\n  \"main\": {\n    \"x\": 2400,\n    \"y\": 300,\n    \"width\": 600,\n    \"height\": 900\n  },\n  \"preference\": {\n    \"x\": -5000,\n    \"y\": 0,\n    \"width\": 960,\n    \"height\": 600\n  }\n}";
        let legacy_path = write_legacy(&paths, legacy_content);
        let monitors = [
            LegacyMonitor {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                scale: 1.0,
            },
            LegacyMonitor {
                x: 1920,
                y: 0,
                width: 3840,
                height: 2160,
                scale: 1.5,
            },
        ];

        let store = WindowStateStore::new(&paths).unwrap();
        assert!(store.has_pending_legacy());
        assert_eq!(store.get("main"), None);

        let migrated = store
            .migrate_legacy(|_, state| legacy_to_logical(state, &monitors))
            .unwrap();
        assert_eq!(migrated, 1);
        assert_eq!(
            store.get("main"),
            Some(WindowGeometry {
                x: 1600.0,
                y: 200.0,
                width: 400.0,
                height: 600.0,
                scale: 1.5,
            })
        );
        // 显示器已拔掉的窗口不迁移，宿主按默认位置摆放。
        assert_eq!(store.get("preference"), None);
        assert!(!store.has_pending_legacy());
        assert_eq!(store.migrate_legacy(|_, _| None).unwrap(), 0);

        store.save("main", geometry(1.0, 2.0)).unwrap();
        assert_eq!(fs::read_to_string(&legacy_path).unwrap(), legacy_content);

        // 原生存档已存在，再启动不会重读 1.x 存档。
        let reopened = WindowStateStore::new(&paths).unwrap();
        assert!(!reopened.has_pending_legacy());
        assert_eq!(reopened.get("main"), Some(geometry(1.0, 2.0)));
    }

    #[test]
    fn migration_writes_native_file_even_when_nothing_converts() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(&temp);
        write_legacy(&paths, "not json");

        let store = WindowStateStore::new(&paths).unwrap();
        assert!(store.has_pending_legacy());
        assert_eq!(store.migrate_legacy(|_, _| None).unwrap(), 0);

        assert!(!WindowStateStore::new(&paths).unwrap().has_pending_legacy());
    }

    #[test]
    fn missing_files_mean_no_state_and_no_migration() {
        let temp = tempfile::tempdir().unwrap();
        let store = WindowStateStore::new(&paths(&temp)).unwrap();

        assert!(!store.has_pending_legacy());
        assert_eq!(store.get("main"), None);
    }

    #[test]
    fn legacy_conversion_uses_the_monitor_holding_the_top_left_corner() {
        let monitors = [
            LegacyMonitor {
                x: -2560,
                y: 0,
                width: 2560,
                height: 1440,
                scale: 2.0,
            },
            LegacyMonitor {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
                scale: 1.25,
            },
        ];
        let state = LegacyWindowState {
            x: -1000,
            y: 100,
            width: 800,
            height: 1200,
        };

        assert_eq!(
            legacy_to_logical(&state, &monitors),
            Some(WindowGeometry {
                x: -500.0,
                y: 50.0,
                width: 400.0,
                height: 600.0,
                scale: 2.0,
            })
        );
        // 右边界与下边界不属于这块屏。
        let edge = LegacyWindowState { x: 1920, ..state };
        assert_eq!(legacy_to_logical(&edge, &monitors), None);
        let bad_scale = [LegacyMonitor {
            scale: 0.0,
            ..monitors[1]
        }];
        let inside = LegacyWindowState { x: 10, ..state };
        assert_eq!(legacy_to_logical(&inside, &bad_scale), None);
    }

    #[test]
    fn rebase_reloads_from_the_new_data_dir() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(&temp);
        let store = WindowStateStore::new(&paths).unwrap();
        store.save("main", geometry(1.0, 1.0)).unwrap();

        let custom = paths.custom_data_dir(&temp.path().join("elsewhere"));
        paths.set_app_data_dir(custom.clone()).unwrap();
        store.rebase(&paths).unwrap();
        assert_eq!(store.get("main"), None);

        store.save("main", geometry(3.0, 4.0)).unwrap();
        assert!(custom.join("state").join(NATIVE_STATE_FILENAME).is_file());
    }
}

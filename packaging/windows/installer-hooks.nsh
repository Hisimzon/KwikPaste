; KwikPaste 2.x 安装包的 NSIS 钩子，配合 Tauri CLI 2.11.4（tauri-bundler 2.9.4）自带的模板。
; tauri.conf.json 的 bundle.windows.nsis.installerHooks 指向这里；模板在 utils.nsh 之后、
; 所有 !define 之前以全局作用域 !include 本文件。
;
; 安装身份（identifier、产品名、注册表键、安装目录、快捷方式、文件关联）全部由模板按配置生成，
; 与 1.x 相同；这里只补三件模板没有的事：
;   1. .onInit 里检查系统版本（Windows 10 1803，build 17134，GPUI 的实际下限），不满足就提示并退出；
;   2. 保住开机自启：界面安装默认「安装前卸载旧版」，旧版卸载程序会删掉 HKCU Run 里的自启项；
;   3. 删掉 1.2.0 / 1.3.0 装在安装目录里的 assets\tray.ico。
; 关掉正在运行的 1.x / 2.x 由模板的 CheckIfAppIsRunning 负责：两者的进程名都是 KwikPaste.exe。

!ifndef KWIKPASTE_MINBUILD
  !define KWIKPASTE_MINBUILD 17134
!endif

!define KWIKPASTE_RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"

; 安装开始时（还没卸载旧版）自启项的值；空表示原本就没开自启。
Var KwikPasteAutostart

; 模板的 .onInit 没有钩子。它（以及 un.onInit）开头都会插入 SetContext，这里把这个宏换成
; 「原样的 SetContext + 安装程序自己的初始化」，卸载程序照常运行。
; 原宏的正文照抄 tauri-bundler 2.9.4 的 utils.nsh；scripts/ci/check-nsis-render.mjs 会核对
; 渲染出的 utils.nsh 里原宏没变、installer.nsi 的 .onInit 仍然插入 SetContext。
!macroundef SetContext
!macro SetContext
  !if "${INSTALLMODE}" == "currentUser"
    SetShellVarContext current
  !else if "${INSTALLMODE}" == "perMachine"
    SetShellVarContext all
  !endif

  ${If} ${RunningX64}
    !if "${ARCH}" == "x64"
      SetRegView 64
    !else if "${ARCH}" == "arm64"
      SetRegView 64
    !else
      SetRegView 32
    !endif
  ${EndIf}

  !ifndef __UNINSTALL__
    !insertmacro KwikPasteRequireWindowsBuild
    ReadRegStr $KwikPasteAutostart HKCU "${KWIKPASTE_RUN_KEY}" "${PRODUCTNAME}"
  !endif
!macroend

; 读注册表里的 build 号而不用 WinVer.nsh：兼容性清单可能让 GetVersion 系列“说谎”。
; 静默安装不弹框，只以 1150（ERROR_OLD_WIN_VERSION）退出。
!macro KwikPasteRequireWindowsBuild
  ReadRegStr $0 HKLM "SOFTWARE\Microsoft\Windows NT\CurrentVersion" "CurrentBuildNumber"
  ${If} $0 < ${KWIKPASTE_MINBUILD}
    ${IfNot} ${Silent}
      ${If} $LANGUAGE = 2052
        MessageBox MB_ICONSTOP "快贴 2 需要 Windows 10 1803 或更高版本，当前系统可以继续使用快贴 1.4。"
      ${Else}
        MessageBox MB_ICONSTOP "KwikPaste 2 requires Windows 10 version 1803 or later. You can keep using KwikPaste 1.4."
      ${EndIf}
    ${EndIf}
    SetErrorLevel 1150
    Quit
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; 1.2.0 / 1.3.0 在安装目录里放过托盘图标，2.x 不再需要。
  Delete "$INSTDIR\assets\tray.ico"
  RMDir "$INSTDIR\assets"

  ; 安装前开着自启、旧版卸载程序又把它删了：按 1.x 的写法指回这次安装的 exe。
  ReadRegStr $0 HKCU "${KWIKPASTE_RUN_KEY}" "${PRODUCTNAME}"
  ${If} $0 == ""
  ${AndIf} $KwikPasteAutostart != ""
    WriteRegStr HKCU "${KWIKPASTE_RUN_KEY}" "${PRODUCTNAME}" '"$INSTDIR\${MAINBINARYNAME}.exe" --auto-launch'
  ${EndIf}
!macroend

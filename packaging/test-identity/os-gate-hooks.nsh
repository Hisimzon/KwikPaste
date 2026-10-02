; 系统版本闸门的负向测试：把下限抬到不存在的 build 99999，安装程序在任何机器上都必须以 1150 退出、
; 不动任何文件。只和 packaging/test-identity/tauri.conf.json 一起用。
!define KWIKPASTE_MINBUILD 99999
!include "${__FILEDIR__}\..\windows\installer-hooks.nsh"

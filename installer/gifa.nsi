; GIFA 安装器（NSIS 3.x）
;
; 设计取舍：
;  * **按用户安装**（$LOCALAPPDATA\Programs\GIFA），不写 HKLM、不触发 UAC。
;    程序本身启动时才要管理员（热键需要），不该让"安装"这一步也卡在 UAC 上。
;  * 卸载**默认保留** %APPDATA%\原神跟跑助手（用户的标定/布局/上次看到都在里面），
;    想清干净需要在卸载页勾选。
;  * `installed.flag` 决定数据目录走向：有它 → %APPDATA%，没有 → exe 同级 data\。

Unicode true
!include "MUI2.nsh"
!include "FileFunc.nsh"

!define APP_NAME "GIFA 原神跟跑助手"
!define APP_ID "GIFA"
!define VERSION "0.2.6"
!define PUBLISHER "GIFA 项目"
!define WEB_URL "https://github.com/civl/GIFA_RS"

Name "${APP_NAME}"
OutFile "..\dist\GIFA-${VERSION}-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\${APP_ID}"
InstallDirRegKey HKCU "Software\${APP_ID}" "InstallDir"
RequestExecutionLevel user
; 不用 /SOLID：本机实测 solid lzma 的安装包在 /S 静默安装下直接退出（127ms、什么都不做），
; 非 solid 的 lzma 正常。体积差 ~10 万字节，换可靠性很值。
SetCompressor lzma
ShowInstDetails show
ShowUninstDetails show

!define MUI_ICON "..\..\reference\GIFA\build\icon.ico"
!define MUI_UNICON "..\..\reference\GIFA\build\icon.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\GIFA.exe"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\..\reference\GIFA\LICENSE"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_SHOWREADME "$INSTDIR\README.txt"
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"

LangString DESC_Main     ${LANG_SIMPCHINESE} "主程序与运行库说明"
LangString DESC_Shortcut ${LANG_SIMPCHINESE} "开始菜单与桌面快捷方式"
LangString DESC_Autostart ${LANG_SIMPCHINESE} "开机自动启动（写入当前用户 HKCU\...\Run）"

Section "主程序" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "..\dist\GIFA-${VERSION}-win-x64\GIFA.exe"
  File "..\dist\GIFA-${VERSION}-win-x64\README.txt"
  ; installed.flag：只作为"安装版"标记，决定数据目录去 %APPDATA%
  FileOpen $0 "$INSTDIR\installed.flag" w
  FileWrite $0 "installed"
  FileClose $0

  SetOutPath "$INSTDIR\licenses"
  File "..\dist\GIFA-${VERSION}-win-x64\licenses\LICENSE-GIFA.txt"
  File "..\dist\GIFA-${VERSION}-win-x64\licenses\NOTICE-GIFA.txt"
  File "..\dist\GIFA-${VERSION}-win-x64\licenses\LICENSE-LGPL-3.0.txt"

  WriteRegStr HKCU "Software\${APP_ID}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\${APP_ID}" "Version" "${VERSION}"

  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "DisplayIcon" "$INSTDIR\GIFA.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "URLInfoAbout" "${WEB_URL}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "NoRepair" 1

  WriteUninstaller "$INSTDIR\uninstall.exe"
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}" "EstimatedSize" "$0"
SectionEnd

Section "快捷方式" SecShortcut
  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\GIFA.exe" "" "$INSTDIR\GIFA.exe" 0
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk" "$INSTDIR\uninstall.exe"
  CreateShortCut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\GIFA.exe" "" "$INSTDIR\GIFA.exe" 0
SectionEnd

Section /o "开机自启" SecAutostart
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${APP_ID}" '"$INSTDIR\GIFA.exe"'
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} $(DESC_Main)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecShortcut} $(DESC_Shortcut)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecAutostart} $(DESC_Autostart)
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  Delete "$DESKTOP\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"

  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${APP_ID}"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}"
  DeleteRegKey HKCU "Software\${APP_ID}"

  Delete "$INSTDIR\GIFA.exe"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\installed.flag"
  Delete "$INSTDIR\uninstall.exe"
  Delete "$INSTDIR\licenses\LICENSE-GIFA.txt"
  Delete "$INSTDIR\licenses\NOTICE-GIFA.txt"
  Delete "$INSTDIR\licenses\LICENSE-LGPL-3.0.txt"
  RMDir "$INSTDIR\licenses"
  RMDir "$INSTDIR"

  ; 默认保留用户数据（标定/布局/上次看到），问一句再删；静默模式一律保留
  IfSilent keepdata
  MessageBox MB_YESNO|MB_ICONQUESTION \
    "是否同时删除配置与标定数据？$\r$\n位置：%APPDATA%\原神跟跑助手$\r$\n$\r$\n选「否」会保留，重装后还能接着用。" \
    IDNO keepdata
  RMDir /r "$APPDATA\原神跟跑助手"
  keepdata:
SectionEnd

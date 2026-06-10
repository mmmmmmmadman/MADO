; installer_win.iss — MADO Windows Inno Setup 安裝腳本（自包含 Python runtime）
; 照 VisionMod installer.iss 範式精簡：
;   - MADO 無 SD 模型 / 無 disk spanning（venv ~176MB + base python ~60MB，遠低於 4.2GB 上限）
;   - 自包含：bundle 可重定位 .venv（cv2 + numpy + pygrabber）+ base python + scripts + ffmpeg/rtaudio DLL
;
; ── 編譯前必備 ──
; 1. cargo build --release（產 target\release\mado.exe，已嵌入 mado.ico）
; 2. ffmpeg/rtaudio runtime DLL 已在 target\release\（avcodec/avformat/avfilter/avutil/
;    swscale/swresample/avdevice/rtaudio）
; 3. dist\.venv 為可重定位 venv（含 base\，pyvenv.cfg 的 home 安裝後改寫為絕對路徑）
; 4. 編譯：ISCC packaging\installer_win.iss（於專案根目錄）
;    ISCC = C:\Users\mmmmm\AppData\Local\Programs\Inno Setup 6\ISCC.exe

#define MyAppName "MADO"
#define MyAppVersion "0.2.0"
#define MyAppExe "mado.exe"
#define DistVenv "dist\.venv"

[Setup]
; .iss 位於 packaging\，但所有來源路徑（assets / target / dist / scripts）相對專案根。
; SourceDir=.. 使 ISCC 以專案根為相對路徑基準。
SourceDir=..
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher=MADZINE
AppPublisherURL=https://github.com/mmmmmmmadman/MADO
DefaultDirName={autopf}\MADZINE\MADO
DefaultGroupName=MADZINE\MADO
UninstallDisplayIcon={app}\{#MyAppExe}
OutputDir=installer_output
OutputBaseFilename=MADO-v{#MyAppVersion}-Windows-Setup
SetupIconFile=assets\icons\mado.ico
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Files]
; 主程式（已嵌入 mado.ico）
Source: "target\release\{#MyAppExe}"; DestDir: "{app}"; Flags: ignoreversion
; 圖示（installer / 捷徑用）
Source: "assets\icons\mado.ico"; DestDir: "{app}"; Flags: ignoreversion
; ffmpeg / rtaudio runtime DLL（exe 動態連結）
Source: "target\release\*.dll"; DestDir: "{app}"; Flags: ignoreversion
; MSVC C/C++ runtime（mado.exe 動態依賴 MSVCP140 / VCRUNTIME140 / VCRUNTIME140_1）。
; 乾淨機器未裝 VC++ Redist 會缺這些 DLL → app 開不起來。bundle 官方 VC143.CRT 整組
; （含 concrt140 等 transitive 依賴）到 {app}，DLL 搜尋順序 exe 同層優先，免裝 redist。
; api-ms-win-crt-* 屬 Windows 10/11 內建 UCRT，不需 bundle。
Source: "packaging\runtime\*.dll"; DestDir: "{app}"; Flags: ignoreversion
; 自包含 Python runtime（可重定位 venv，含 base\；Scripts\python.exe 走 pyvenv.cfg home）
Source: "{#DistVenv}\*"; DestDir: "{app}\.venv"; Flags: ignoreversion recursesubdirs createallsubdirs
; Python camera service 腳本（Windows cv2 + pygrabber + frame_mmap）
Source: "scripts\*"; DestDir: "{app}\scripts"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\MADO"; Filename: "{app}\{#MyAppExe}"; IconFilename: "{app}\mado.ico"
Name: "{autodesktop}\MADO"; Filename: "{app}\{#MyAppExe}"; IconFilename: "{app}\mado.ico"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional icons:"

[Run]
Filename: "{app}\{#MyAppExe}"; Description: "Launch MADO"; Flags: nowait postinstall skipifsilent

[Code]
// 自包含 venv = 重定位 venv + bundle base Python。
// venv 的 Scripts\python.exe 透過 pyvenv.cfg 的 home 指向 base 解析 stdlib / python310.dll。
// home 必須是絕對路徑（相對路徑經 VisionMod 實測無效，會以 CWD 解析），安裝目錄編譯期未知，
// 故安裝後改寫 pyvenv.cfg 的 home 為 .venv\base 的絕對路徑。
procedure CurStepChanged(CurStep: TSetupStep);
var
  CfgPath: String;
  Lines: TArrayOfString;
begin
  if CurStep = ssPostInstall then
  begin
    CfgPath := ExpandConstant('{app}\.venv\pyvenv.cfg');
    SetArrayLength(Lines, 3);
    Lines[0] := 'home = ' + ExpandConstant('{app}\.venv\base');
    Lines[1] := 'include-system-site-packages = false';
    Lines[2] := 'version = 3.10.11';
    SaveStringsToFile(CfgPath, Lines, False);
  end;
end;

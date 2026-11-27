#include "tray.h"

#include <shellapi.h>

#include <cwchar>

#include "core.h"
#include "ui.h"

namespace {
constexpr UINT kId = 1;
constexpr UINT kOpen = 1001, kExit = 1002;
const wchar_t* kText = L"Domain OS Toolbox";

void Copy(wchar_t* dst, size_t cap, const std::wstring& s)
{
	size_t n = s.size() < cap - 1 ? s.size() : cap - 1;
	wmemcpy(dst, s.c_str(), n);
	dst[n] = 0;
}
} // namespace

Tray::Tray(HWND owner, std::function<void()> open, std::function<void()> exit)
	: hwnd_(owner), open_(std::move(open)), exit_(std::move(exit))
{
	// The same icon as the window (app.ico, resource 1), at the tray's own size (SystemInformation.SmallIconSize).
	HINSTANCE inst = GetModuleHandleW(nullptr);
	icon_ = (HICON)LoadImageW(inst, MAKEINTRESOURCEW(1), IMAGE_ICON, GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON), LR_DEFAULTCOLOR);
	if (icon_ == nullptr) icon_ = LoadIconW(nullptr, IDI_APPLICATION);
	taskbarCreated_ = RegisterWindowMessageW(L"TaskbarCreated");
	Add();
}

Tray::~Tray()
{
	NOTIFYICONDATAW d{};
	d.cbSize = sizeof d;
	d.hWnd = hwnd_;
	d.uID = kId;
	Shell_NotifyIconW(NIM_DELETE, &d);
	if (icon_ != nullptr) DestroyIcon(icon_);
}

void Tray::Add()
{
	NOTIFYICONDATAW d{};
	d.cbSize = sizeof d;
	d.hWnd = hwnd_;
	d.uID = kId;
	d.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
	d.uCallbackMessage = WM_APP_TRAY;
	d.hIcon = icon_;
	Copy(d.szTip, sizeof d.szTip / sizeof d.szTip[0], kText);
	added_ = Shell_NotifyIconW(NIM_ADD, &d) != FALSE;
}

void Tray::Balloon(const std::string& text, DWORD icon)
{
	NOTIFYICONDATAW d{};
	d.cbSize = sizeof d;
	d.hWnd = hwnd_;
	d.uID = kId;
	d.uFlags = NIF_INFO;
	d.uTimeout = 5000;
	d.dwInfoFlags = icon;
	Copy(d.szInfoTitle, sizeof d.szInfoTitle / sizeof d.szInfoTitle[0], kText);
	Copy(d.szInfo, sizeof d.szInfo / sizeof d.szInfo[0], ui::Wide(text));
	Shell_NotifyIconW(NIM_MODIFY, &d);
}

void Tray::Notice(const std::string& text) { Balloon(text, NIIF_ERROR); }

void Tray::Info(const std::string& text) { Balloon(text, NIIF_INFO); }

void Tray::Menu()
{
	HMENU m = CreatePopupMenu();
	if (m == nullptr) return;
	AppendMenuW(m, MF_STRING, kOpen, ui::Wide(L::T("Open")).c_str());
	AppendMenuW(m, MF_STRING, kExit, ui::Wide(L::T("Exit")).c_str());
	POINT pt;
	GetCursorPos(&pt);
	SetForegroundWindow(hwnd_);
	UINT cmd = (UINT)TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd_, nullptr);
	PostMessageW(hwnd_, WM_NULL, 0, 0);
	DestroyMenu(m);
	if (cmd == kOpen && open_) open_();
	else if (cmd == kExit && exit_) exit_();
}

bool Tray::Handle(UINT msg, WPARAM wp, LPARAM lp)
{
	if (msg == taskbarCreated_ && taskbarCreated_ != 0)
	{
		Add();   // Explorer restarted: the icon is added again (NotifyIcon does the same)
		return true;
	}
	if (msg != WM_APP_TRAY) return false;
	switch ((UINT)lp)
	{
	case WM_LBUTTONDBLCLK:
		if (open_) open_();
		break;
	case WM_RBUTTONUP:
		Menu();
		break;
	default:
		break;
	}
	return true;
}

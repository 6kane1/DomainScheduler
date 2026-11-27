// Program.cs + the window part of App.xaml.cs: the Win32 window with our own chrome, Direct3D 11 + Dear ImGui,
// the tray, the error popup and the on-demand render loop. Start-up logic (single instance, switches, the long-lived
// objects) is the core's (dos_app_start). Owner: app shell.
//
// Rendering is on demand: the loop blocks in MsgWaitForMultipleObjects until input, a timer a widget asked for
// (app::WakeIn), the Show event or a change of the core's state version; nothing is drawn while the window is
// hidden in the tray or minimized.
//
// Like the C# app (RenderOptions.ProcessRenderMode = RenderMode.SoftwareOnly) the window never uses the graphics
// card: Direct3D 11 runs on WARP, the software rasterizer. Only when WARP cannot be created the hardware device is
// tried, so the window still opens.
#include <windows.h>

#include <d3d11.h>
#include <dxgi.h>
#include <shellapi.h>
#include <windowsx.h>

#include <algorithm>
#include <atomic>
#include <cstring>
#include <cmath>
#include <memory>
#include <cstdio>
#include <cstdlib>
#include <string>
#include <vector>

#include "app.h"
#include "chrome.h"
#include "core.h"
#include "error_popup.h"
#include "imgui.h"
#include "imgui_impl_dx11.h"
#include "imgui_impl_win32.h"
#include "shell.h"
#include "theme.h"
#include "tray.h"
#include "ui.h"

extern IMGUI_IMPL_API LRESULT ImGui_ImplWin32_WndProcHandler(HWND hWnd, UINT msg, WPARAM wParam, LPARAM lParam);

namespace {

constexpr UINT WM_APP_REDRAW = WM_APP + 40;

HWND g_hwnd = nullptr;
ID3D11Device* g_dev = nullptr;
ID3D11DeviceContext* g_ctx = nullptr;
IDXGISwapChain* g_swap = nullptr;
ID3D11RenderTargetView* g_rtv = nullptr;
bool g_warp = false;
bool g_quit = false;
bool g_inSizeMove = false;
int g_frames = 0;                 // frames still to draw (ImGui settles hover / layout over a couple of frames)
double g_wakeAt = 0;              // app::Now() of the earliest WakeIn, 0 = none
LARGE_INTEGER g_freq{}, g_t0{};
std::unique_ptr<Shell> g_shell;
std::unique_ptr<Tray> g_tray;
std::unique_ptr<ErrorPopup> g_errors;

void CreateRtv()
{
	ID3D11Texture2D* back = nullptr;
	if (SUCCEEDED(g_swap->GetBuffer(0, IID_PPV_ARGS(&back))) && back)
	{
		g_dev->CreateRenderTargetView(back, nullptr, &g_rtv);
		back->Release();
	}
}

void ReleaseRtv()
{
	if (g_rtv)
	{
		g_rtv->Release();
		g_rtv = nullptr;
	}
}

bool TryDevice(D3D_DRIVER_TYPE type, bool flip)
{
	DXGI_SWAP_CHAIN_DESC sd{};
	sd.BufferCount = 2;
	sd.BufferDesc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
	sd.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
	sd.OutputWindow = g_hwnd;
	sd.SampleDesc.Count = 1;
	sd.Windowed = TRUE;
	sd.SwapEffect = flip ? DXGI_SWAP_EFFECT_FLIP_DISCARD : DXGI_SWAP_EFFECT_DISCARD;
	if (!flip) sd.BufferCount = 1;
	const D3D_FEATURE_LEVEL levels[] = {D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0};
	D3D_FEATURE_LEVEL got;
	HRESULT hr = D3D11CreateDeviceAndSwapChain(nullptr, type, nullptr, 0, levels, 3, D3D11_SDK_VERSION, &sd, &g_swap, &g_dev, &got, &g_ctx);
	return SUCCEEDED(hr) && g_swap && g_dev && g_ctx;
}

bool CreateDevice()
{
	// WARP first (the C# window renders in software); flip model on Windows 10+, blt model where flip is missing.
	const D3D_DRIVER_TYPE order[] = {D3D_DRIVER_TYPE_WARP, D3D_DRIVER_TYPE_HARDWARE};
	for (D3D_DRIVER_TYPE t : order)
		for (bool flip : {true, false})
			if (TryDevice(t, flip))
			{
				g_warp = t == D3D_DRIVER_TYPE_WARP;
				if (IDXGIDevice* dxgi = nullptr; SUCCEEDED(g_dev->QueryInterface(IID_PPV_ARGS(&dxgi))) && dxgi)
				{
					IDXGIAdapter* ad = nullptr;
					if (SUCCEEDED(dxgi->GetAdapter(&ad)) && ad)
					{
						IDXGIFactory* f = nullptr;
						if (SUCCEEDED(ad->GetParent(IID_PPV_ARGS(&f))) && f)
						{
							f->MakeWindowAssociation(g_hwnd, DXGI_MWA_NO_ALT_ENTER);
							f->Release();
						}
						ad->Release();
					}
					dxgi->Release();
				}
				CreateRtv();
				return true;
			}
	return false;
}

void Cleanup()
{
	ReleaseRtv();
	if (g_swap) g_swap->Release();
	if (g_ctx) g_ctx->Release();
	if (g_dev) g_dev->Release();
	g_swap = nullptr;
	g_ctx = nullptr;
	g_dev = nullptr;
}

void Render()
{
	if (!g_dev || !g_rtv) return;
	ImGui_ImplDX11_NewFrame();
	ImGui_ImplWin32_NewFrame();
	ImGui::NewFrame();
	if (g_shell) g_shell->Draw();
	ImGui::Render();
	const float clear[4] = {6 / 255.0f, 6 / 255.0f, 8 / 255.0f, 1.0f};   // Bg
	g_ctx->OMSetRenderTargets(1, &g_rtv, nullptr);
	g_ctx->ClearRenderTargetView(g_rtv, clear);
	ImGui_ImplDX11_RenderDrawData(ImGui::GetDrawData());
	g_swap->Present(1, 0);
	// A text box keeps its caret blinking (the WPF caret blinks too).
	if (ImGui::GetIO().WantTextInput) app::WakeIn(530);
}

std::string ArgsJson()
{
	int n = 0;
	LPWSTR* argv = CommandLineToArgvW(GetCommandLineW(), &n);
	std::string j = "[";
	for (int i = 1; argv && i < n; i++)
	{
		if (i > 1) j += ",";
		j += json::Quote(ui::Utf8(argv[i]));
	}
	if (argv) LocalFree(argv);
	return j + "]";
}

void OnCore()
{
	if (!g_shell) return;
	g_shell->OnCoreChanged();
	json::Value errs = core::TakeJson(dos_log_take_errors());
	for (const auto& e : errs.Arr())
		if (g_errors) g_errors->OnError(ErrorPopup::Entry{e["t"].Str(), e["msg"].Str()});
	json::Value notes = core::TakeJson(dos_tray_take());
	for (const auto& n : notes.Arr())
	{
		if (!g_tray) break;
		if (n["kind"].Str() == "error") g_tray->Notice(n["text"].Str());
		else g_tray->Info(n["text"].Str());
	}
	g_shell->Refresh();   // the Optimizer's Evaluate / Auto.Changed -> Shell.Refresh
}

float WindowScale(HWND h)
{
	UINT d = GetDpiForWindow(h);
	return d ? d / 96.0f : 1.0f;
}

LRESULT CALLBACK WndProc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp)
{
	if (g_tray && g_tray->Handle(msg, wp, lp)) return 0;
	switch (msg)
	{
	case WM_NCCALCSIZE:
		if (wp) return 0;   // the whole window is client area: the title bar is ours (WindowChrome)
		break;
	case WM_NCACTIVATE:
		return DefWindowProcW(hwnd, msg, wp, -1);   // no system frame repaint over our content
	case WM_NCHITTEST:
	{
		POINT pt{GET_X_LPARAM(lp), GET_Y_LPARAM(lp)};
		ScreenToClient(hwnd, &pt);
		float cap = g_shell ? g_shell->CaptionPx() : ui::S(chrome::CaptionHeight);
		return chrome::HitTest(hwnd, pt, cap, IsZoomed(hwnd) != FALSE);
	}
	case WM_GETMINMAXINFO:
	{
		auto* mm = (MINMAXINFO*)lp;
		ImVec2 mn = g_shell ? g_shell->MinSizeDip() : ImVec2(1180, 700);
		float s = WindowScale(hwnd);
		mm->ptMinTrackSize.x = (LONG)std::lround(mn.x * s);
		mm->ptMinTrackSize.y = (LONG)std::lround(mn.y * s);
		return 0;
	}
	case WM_DPICHANGED:
	{
		ui::SetDpi(HIWORD(wp) / 96.0f);
		ui::ScaleStyle();
		const RECT* r = (const RECT*)lp;
		SetWindowPos(hwnd, nullptr, r->left, r->top, r->right - r->left, r->bottom - r->top, SWP_NOZORDER | SWP_NOACTIVATE);
		g_frames = 3;
		return 0;
	}
	case WM_SIZE:
		if (g_swap && wp != SIZE_MINIMIZED)
		{
			ReleaseRtv();
			g_swap->ResizeBuffers(0, (UINT)LOWORD(lp), (UINT)HIWORD(lp), DXGI_FORMAT_UNKNOWN, 0);
			CreateRtv();
		}
		if (g_shell && (wp == SIZE_MINIMIZED || wp == SIZE_RESTORED || wp == SIZE_MAXIMIZED)) g_shell->OnWindowState();
		g_frames = std::max(g_frames, 2);
		if (g_inSizeMove && app::OnScreen()) Render();   // the resize loop is modal: draw from here
		return 0;
	case WM_ENTERSIZEMOVE:
		g_inSizeMove = true;
		break;
	case WM_EXITSIZEMOVE:
		g_inSizeMove = false;
		g_frames = std::max(g_frames, 2);
		break;
	case WM_PAINT:
	{
		PAINTSTRUCT ps;
		BeginPaint(hwnd, &ps);
		EndPaint(hwnd, &ps);
		g_frames = std::max(g_frames, 1);
		if (g_inSizeMove) Render();
		return 0;
	}
	case WM_ERASEBKGND:
		return 1;
	case WM_SHOWWINDOW:
		PostMessageW(hwnd, WM_APP_REDRAW, 1, 0);
		break;
	case WM_APP_REDRAW:
		if (wp == 1 && g_shell) g_shell->OnWindowState();
		g_frames = std::max(g_frames, 2);
		return 0;
	case WM_CLOSE:
		// OnClosing: cancelled; the window goes to the tray.
		if (g_shell) g_shell->SaveGeometry();
		ShowWindow(hwnd, SW_HIDE);
		if (g_shell) g_shell->OnWindowState();
		return 0;
	case WM_QUERYENDSESSION:
		app::ExitApp();   // SessionEnding
		return TRUE;
	case WM_ENDSESSION:
		if (wp) g_quit = true;
		return 0;
	case WM_MOUSEWHEEL:
	{
		// The WPF ScrollViewer moves 48 DIP per notch (3 lines of 16); ImGui moves 5 lines of its font: scale it.
		float notch = (float)GET_WHEEL_DELTA_WPARAM(wp) / (float)WHEEL_DELTA;
		float fontPx = ui::FontPx(ui::Face::Ui, ui::FsBody);
		ImGui::GetIO().AddMouseWheelEvent(0.0f, notch * (48.0f * ui::Dpi()) / std::max(1.0f, 5.0f * fontPx));
		g_frames = std::max(g_frames, 2);
		return 0;
	}
	case WM_DESTROY:
		g_quit = true;
		return 0;
	default:
		break;
	}
	if (ImGui::GetCurrentContext() != nullptr && ImGui_ImplWin32_WndProcHandler(hwnd, msg, wp, lp)) return 1;
	return DefWindowProcW(hwnd, msg, wp, lp);
}

bool IsInput(UINT m)
{
	return (m >= WM_MOUSEFIRST && m <= WM_MOUSELAST) || (m >= WM_KEYFIRST && m <= WM_KEYLAST) || m == WM_MOUSELEAVE ||
	       m == WM_NCMOUSEMOVE || m == WM_NCMOUSELEAVE || m == WM_SETFOCUS || m == WM_KILLFOCUS || m == WM_CHAR || m == WM_ACTIVATE;
}

} // namespace

// ================================================================ app services

namespace app {

HWND Hwnd() { return g_hwnd; }
ID3D11Device* Device() { return g_dev; }

void Redraw()
{
	if (g_hwnd) PostMessageW(g_hwnd, WM_APP_REDRAW, 0, 0);
}

double Now()
{
	LARGE_INTEGER t;
	QueryPerformanceCounter(&t);
	return (double)(t.QuadPart - g_t0.QuadPart) / (double)g_freq.QuadPart;
}

void WakeIn(int ms)
{
	double at = Now() + std::max(0, ms) / 1000.0;
	if (g_wakeAt == 0 || at < g_wakeAt) g_wakeAt = at;
}

#ifdef DOS_UI_SHOT
bool OnScreen() { return true; }
#else
bool OnScreen() { return g_hwnd && IsWindowVisible(g_hwnd) && !IsIconic(g_hwnd); }
#endif

void ShowWindow()
{
	if (!g_hwnd) return;
	::ShowWindow(g_hwnd, SW_SHOW);
	if (IsIconic(g_hwnd)) ::ShowWindow(g_hwnd, SW_RESTORE);
	SetForegroundWindow(g_hwnd);
	if (g_shell) g_shell->OnWindowState();
	g_frames = std::max(g_frames, 2);
}

void ExitApp()
{
	if (dos_app_exit() != 0) g_quit = true;
}

void TrayNotice(const std::string& text)
{
	if (g_tray) g_tray->Notice(text);
}

void TrayInfo(const std::string& text)
{
	if (g_tray) g_tray->Info(text);
}

} // namespace app

#ifdef DOS_UI_SHOT
// ================================================================ test aid (not in the product exe)
// Built only with -DDOS_UI_SHOT: draws the shell into an offscreen WARP texture and writes it as a BMP, for
// looking at the UI where no display exists (wine without X). DOS_SHOT_OUT=file.bmp DOS_SHOT_W/H, DOS_SHOT_SCALE,
// DOS_SHOT_MOUSE=x,y (hover), DOS_SHOT_CLICK=x,y (a click before the shot).
static int EnvInt(const char* n, int def)
{
	const char* v = getenv(n);
	return v && *v ? atoi(v) : def;
}

// A tiny CPU rasterizer for ImGui's draw data (wine without a display has no Direct3D at all).
struct ShotTex {
	int w = 0, h = 0;
	std::vector<unsigned char> rgba;
};
static std::vector<ShotTex> g_shotTex(1);

static void ShotTextures(ImDrawData* dd)
{
	if (!dd->Textures) return;
	for (ImTextureData* t : *dd->Textures)
	{
		if (t->Status == ImTextureStatus_WantCreate || t->Status == ImTextureStatus_WantUpdates)
		{
			size_t id = t->TexID == ImTextureID_Invalid ? g_shotTex.size() : (size_t)t->TexID;
			if (id >= g_shotTex.size()) g_shotTex.resize(id + 1);
			ShotTex& st = g_shotTex[id];
			st.w = t->Width;
			st.h = t->Height;
			st.rgba.assign((size_t)st.w * st.h * 4, 0);
			const unsigned char* src = (const unsigned char*)t->GetPixels();
			for (int i = 0; i < st.w * st.h; i++)
			{
				if (t->Format == ImTextureFormat_RGBA32) memcpy(&st.rgba[(size_t)i * 4], src + i * 4, 4);
				else { st.rgba[(size_t)i * 4] = st.rgba[(size_t)i * 4 + 1] = st.rgba[(size_t)i * 4 + 2] = 255; st.rgba[(size_t)i * 4 + 3] = src[i]; }
			}
			t->SetTexID((ImTextureID)id);
			t->SetStatus(ImTextureStatus_OK);
		}
		else if (t->Status == ImTextureStatus_WantDestroy && t->UnusedFrames > 0)
		{
			t->SetTexID(ImTextureID_Invalid);
			t->SetStatus(ImTextureStatus_Destroyed);
		}
	}
}

static void ShotRaster(ImDrawData* dd, std::vector<float>& fb, int W, int H)
{
	for (int n = 0; n < dd->CmdListsCount; n++)
	{
		const ImDrawList* cl = dd->CmdLists[n];
		for (const ImDrawCmd& cmd : cl->CmdBuffer)
		{
			if (cmd.UserCallback) continue;
			size_t tid = (size_t)cmd.GetTexID();
			const ShotTex* tex = tid < g_shotTex.size() ? &g_shotTex[tid] : nullptr;
			int cx0 = std::max(0, (int)cmd.ClipRect.x), cy0 = std::max(0, (int)cmd.ClipRect.y);
			int cx1 = std::min(W, (int)std::ceil(cmd.ClipRect.z)), cy1 = std::min(H, (int)std::ceil(cmd.ClipRect.w));
			for (unsigned e = 0; e + 2 < cmd.ElemCount; e += 3)
			{
				const ImDrawVert* v[3];
				for (int k = 0; k < 3; k++) v[k] = &cl->VtxBuffer[cmd.VtxOffset + cl->IdxBuffer[cmd.IdxOffset + e + k]];
				float x0 = v[0]->pos.x, y0 = v[0]->pos.y, x1 = v[1]->pos.x, y1 = v[1]->pos.y, x2 = v[2]->pos.x, y2 = v[2]->pos.y;
				float area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
				if (std::fabs(area) < 1e-6f) continue;
				int bx0 = std::max(cx0, (int)std::floor(std::min({x0, x1, x2}))), bx1 = std::min(cx1, (int)std::ceil(std::max({x0, x1, x2})));
				int by0 = std::max(cy0, (int)std::floor(std::min({y0, y1, y2}))), by1 = std::min(cy1, (int)std::ceil(std::max({y0, y1, y2})));
				for (int y = by0; y < by1; y++)
					for (int x = bx0; x < bx1; x++)
					{
						float px = x + 0.5f, py = y + 0.5f;
						float w0 = ((x1 - px) * (y2 - py) - (x2 - px) * (y1 - py)) / area;
						float w1 = ((x2 - px) * (y0 - py) - (x0 - px) * (y2 - py)) / area;
						float w2 = 1 - w0 - w1;
						if (w0 < -1e-5f || w1 < -1e-5f || w2 < -1e-5f) continue;
						float c[4];
						for (int k = 0; k < 4; k++)
						{
							int sh = k * 8;
							c[k] = (w0 * ((v[0]->col >> sh) & 0xFF) + w1 * ((v[1]->col >> sh) & 0xFF) + w2 * ((v[2]->col >> sh) & 0xFF)) / 255.0f;
						}
						if (tex && tex->w > 0)
						{
							float u = w0 * v[0]->uv.x + w1 * v[1]->uv.x + w2 * v[2]->uv.x, vv = w0 * v[0]->uv.y + w1 * v[1]->uv.y + w2 * v[2]->uv.y;
							float fx = u * tex->w - 0.5f, fy = vv * tex->h - 0.5f;
							int ix = (int)std::floor(fx), iy = (int)std::floor(fy);
							float ax = fx - ix, ay = fy - iy;
							float t[4] = {0, 0, 0, 0};
							for (int q = 0; q < 4; q++)
							{
								int sx = std::clamp(ix + (q & 1), 0, tex->w - 1), sy = std::clamp(iy + (q >> 1), 0, tex->h - 1);
								float wq = ((q & 1) ? ax : 1 - ax) * ((q >> 1) ? ay : 1 - ay);
								const unsigned char* tp = &tex->rgba[((size_t)sy * tex->w + sx) * 4];
								for (int k = 0; k < 4; k++) t[k] += wq * tp[k] / 255.0f;
							}
							for (int k = 0; k < 4; k++) c[k] *= t[k];
						}
						float* d = &fb[((size_t)y * W + x) * 3];
						for (int k = 0; k < 3; k++) d[k] = c[k] * c[3] + d[k] * (1 - c[3]);
					}
			}
		}
	}
}

static int ShotMain()
{
	int w = EnvInt("DOS_SHOT_W", 1340), h = EnvInt("DOS_SHOT_H", 840);
	const char* scaleS = getenv("DOS_SHOT_SCALE");
	ui::SetDpi(scaleS && *scaleS ? (float)atof(scaleS) : 1.0f);
	w = (int)std::lround(w * ui::Dpi());
	h = (int)std::lround(h * ui::Dpi());
	int show = 1;
	dos_app_start("[]", &show);
	ImGui::CreateContext();
	ImGuiIO& io = ImGui::GetIO();
	io.DisplaySize = ImVec2((float)w, (float)h);
	io.BackendFlags |= ImGuiBackendFlags_RendererHasTextures | ImGuiBackendFlags_RendererHasVtxOffset;
	ui::ApplyTheme();
	g_shell = std::make_unique<Shell>(nullptr);
	SetTheShell(g_shell.get());
	if (const char* a = getenv("DOS_SHOT_SIMPLE")) g_shell->SetSimple(*a == '1', false);
	if (const char* pg = getenv("DOS_SHOT_PAGE")) g_shell->Show(atoi(pg));
	if (getenv("DOS_SHOT_AUTO")) { dos_auto_click(); }
	int mx = -1, my = -1, cx = -1, cy = -1;
	if (const char* m = getenv("DOS_SHOT_MOUSE")) sscanf(m, "%d,%d", &mx, &my);
	if (const char* c = getenv("DOS_SHOT_CLICK")) sscanf(c, "%d,%d", &cx, &cy);
	std::vector<float> fb;
	// DOS_SHOT_FRAMES: more frames = more animation time (0.25 s each) before the shot (the Thread map's pulses).
	int frames = std::max(8, EnvInt("DOS_SHOT_FRAMES", 8));
	for (int f = 0; f < frames; f++)
	{
		io.DeltaTime = 0.25f;
		if (cx >= 0 && f == 1) { io.AddMousePosEvent((float)cx, (float)cy); io.AddMouseButtonEvent(0, true); }
		else if (cx >= 0 && f == 2) io.AddMouseButtonEvent(0, false);
		else if (mx >= 0 && f >= 3) io.AddMousePosEvent((float)mx, (float)my);
		if (f == 3 || f == 6) g_shell->Refresh();
		// DOS_SHOT_SLEEP_MS: real time per frame, so jobs the page started (process list ...) can finish.
		if (int ms = EnvInt("DOS_SHOT_SLEEP_MS", 0); ms > 0) Sleep((DWORD)ms);
		g_shell->PollJobs();
		ImGui::NewFrame();
		g_shell->Draw();
		ImGui::Render();
		ShotTextures(ImGui::GetDrawData());
		fb.assign((size_t)w * h * 3, 0.0f);
		for (size_t i = 0; i < (size_t)w * h; i++) { fb[i * 3] = 6 / 255.0f; fb[i * 3 + 1] = 6 / 255.0f; fb[i * 3 + 2] = 8 / 255.0f; }
		if (f == frames - 1) ShotRaster(ImGui::GetDrawData(), fb, w, h);
	}
	const char* out = getenv("DOS_SHOT_OUT");
	FILE* fp = fopen(out && *out ? out : "shot.bmp", "wb");
	if (!fp) return 4;
	BITMAPFILEHEADER fh{};
	BITMAPINFOHEADER bh{};
	bh.biSize = sizeof bh;
	bh.biWidth = w;
	bh.biHeight = -h;
	bh.biPlanes = 1;
	bh.biBitCount = 32;
	fh.bfType = 0x4D42;
	fh.bfOffBits = sizeof fh + sizeof bh;
	fh.bfSize = fh.bfOffBits + (DWORD)(w * h * 4);
	fwrite(&fh, sizeof fh, 1, fp);
	fwrite(&bh, sizeof bh, 1, fp);
	std::vector<unsigned char> row((size_t)w * 4);
	for (int y = 0; y < h; y++)
	{
		for (int x = 0; x < w; x++)
		{
			const float* p = &fb[((size_t)y * w + x) * 3];
			row[(size_t)x * 4] = (unsigned char)std::lround(std::clamp(p[2], 0.0f, 1.0f) * 255);
			row[(size_t)x * 4 + 1] = (unsigned char)std::lround(std::clamp(p[1], 0.0f, 1.0f) * 255);
			row[(size_t)x * 4 + 2] = (unsigned char)std::lround(std::clamp(p[0], 0.0f, 1.0f) * 255);
			row[(size_t)x * 4 + 3] = 255;
		}
		fwrite(row.data(), 1, row.size(), fp);
	}
	fclose(fp);
	return 0;
}
#endif

// ================================================================ entry point

int WINAPI wWinMain(HINSTANCE inst, HINSTANCE, PWSTR, int)
{
	QueryPerformanceFrequency(&g_freq);
	QueryPerformanceCounter(&g_t0);
#ifdef DOS_UI_SHOT
	return ShotMain();
#endif
	SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);   // also in app.manifest
	int show = 1;
	int rc = dos_app_start(ArgsJson().c_str(), &show);
	if (rc >= 0) return rc;

	WNDCLASSEXW wc{};
	wc.cbSize = sizeof wc;
	wc.style = CS_HREDRAW | CS_VREDRAW;
	wc.lpfnWndProc = WndProc;
	wc.hInstance = inst;
	wc.hIcon = LoadIconW(inst, MAKEINTRESOURCEW(1));
	wc.hIconSm = (HICON)LoadImageW(inst, MAKEINTRESOURCEW(1), IMAGE_ICON, GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON), 0);
	wc.hCursor = LoadCursorW(nullptr, IDC_ARROW);
	wc.lpszClassName = L"DomainOSToolbox";
	RegisterClassExW(&wc);
	// Hidden at first; 1340 x 840 DIP (the advanced size; SetSimple sets the simple one).
	g_hwnd = CreateWindowExW(0, wc.lpszClassName, L"Domain OS", WS_OVERLAPPEDWINDOW, CW_USEDEFAULT, CW_USEDEFAULT, 1340, 840, nullptr, nullptr, inst, nullptr);
	if (!g_hwnd) return 1;
	ui::SetDpi(WindowScale(g_hwnd));
	SetWindowPos(g_hwnd, nullptr, 0, 0, (int)std::lround(1340 * ui::Dpi()), (int)std::lround(840 * ui::Dpi()), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
	if (!CreateDevice())
	{
		Cleanup();
		MessageBoxW(nullptr, L"Direct3D 11 is not available.", L"Domain OS Toolbox", MB_OK | MB_ICONERROR);
		return 1;
	}
	if (!g_warp) core::Log("UI: software rendering (WARP) is not available, the graphics card draws the window", core::Sev::Warn);

	IMGUI_CHECKVERSION();
	ImGui::CreateContext();
	ImGui_ImplWin32_Init(g_hwnd);
	ImGui_ImplDX11_Init(g_dev, g_ctx);
	ui::ApplyTheme();

	g_shell = std::make_unique<Shell>(g_hwnd);
	SetTheShell(g_shell.get());
	g_shell->OnCreated();
	g_errors = std::make_unique<ErrorPopup>(
		[] { return core::TakeJson(dos_shell_state())["game_running"].Bool(); },
		[] { return g_hwnd; },
		[](const std::string& t) { app::TrayNotice(t); });
	g_tray = std::make_unique<Tray>(g_hwnd, [] { app::ShowWindow(); }, [] { app::ExitApp(); });
	g_shell->RestoreGeometry();
	if (show) app::ShowWindow();
	else g_shell->OnWindowState();

	HANDLE handles[2];
	DWORD n = 0;
	HANDLE showEv = (HANDLE)dos_show_event();
	HANDLE stateEv = (HANDLE)dos_state_event();
	DWORD showIdx = (DWORD)-1, stateIdx = (DWORD)-1;
	if (showEv) { showIdx = n; handles[n++] = showEv; }
	if (stateEv) { stateIdx = n; handles[n++] = stateEv; }
	uint64_t version = dos_state_version();
	g_frames = 2;
	while (!g_quit)
	{
		DWORD timeout = INFINITE;
		double now = app::Now();
		if (g_wakeAt > 0) timeout = (DWORD)std::max(0.0, std::ceil((g_wakeAt - now) * 1000.0));
		if (!stateEv) timeout = std::min<DWORD>(timeout, 250);   // no event from the core: poll the version
		if (g_frames > 0 && app::OnScreen()) timeout = 0;
		DWORD w = MsgWaitForMultipleObjectsEx(n, handles, timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
		if (showIdx != (DWORD)-1 && w == WAIT_OBJECT_0 + showIdx) app::ShowWindow();
		MSG m;
		while (PeekMessageW(&m, nullptr, 0, 0, PM_REMOVE))
		{
			if (m.message == WM_QUIT) g_quit = true;
			if (IsInput(m.message)) g_frames = std::max(g_frames, 3);
			TranslateMessage(&m);
			DispatchMessageW(&m);
		}
		if (g_quit) break;
		uint64_t v = dos_state_version();
		if (v != version)
		{
			version = v;
			OnCore();
			g_frames = std::max(g_frames, 2);
		}
		g_shell->PollJobs();
		now = app::Now();
		g_errors->Poll(now);
		if (g_wakeAt > 0 && now >= g_wakeAt)
		{
			g_wakeAt = 0;
			g_frames = std::max(g_frames, 2);
		}
		if (g_frames > 0)
		{
			if (app::OnScreen())
			{
				Render();
				g_frames--;
			}
			else
				g_frames = 0;
		}
	}
	(void)stateIdx;
	if (g_shell) g_shell->SaveGeometry();   // App.CleanUp: _window?.SaveGeometry() on every way out
	g_tray.reset();
	g_errors.reset();
	SetTheShell(nullptr);
	g_shell.reset();
	ImGui_ImplDX11_Shutdown();
	ImGui_ImplWin32_Shutdown();
	ImGui::DestroyContext();
	Cleanup();
	if (g_hwnd) DestroyWindow(g_hwnd);
	return 0;
}

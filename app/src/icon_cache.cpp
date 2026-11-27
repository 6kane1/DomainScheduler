#include "icon_cache.h"

#include <windows.h>

#include <objbase.h>
#include <shellapi.h>

#include <d3d11.h>

#include <memory>
#include <mutex>
#include <unordered_map>
#include <vector>

#include "app.h"
#include "ui.h"

namespace icons {

namespace {

struct Item {
	bool ready = false;   // the read finished (pixels may be empty = no icon)
	int size = 0;
	std::vector<unsigned char> rgba;
	ID3D11ShaderResourceView* srv = nullptr;
	bool uploaded = false;
};

std::mutex g_lock;
// The key ignores case like the C# ConcurrentDictionary(StringComparer.OrdinalIgnoreCase).
std::unordered_map<std::wstring, std::shared_ptr<Item>> g_cache;

std::wstring Key(const std::string& path)
{
	std::wstring w = ui::Wide(path);
	if (!w.empty()) CharUpperBuffW(w.data(), (DWORD)w.size());
	return w;
}

// Icon.ExtractAssociatedIcon, then CreateBitmapSourceFromHIcon at pixels x pixels (straight alpha RGBA).
std::vector<unsigned char> Read(const std::wstring& path, int px)
{
	std::vector<unsigned char> out;
	wchar_t buf[MAX_PATH * 2];
	if (path.size() >= MAX_PATH * 2) return out;
	wcscpy_s(buf, path.c_str());
	WORD index = 0;
	HICON icon = ExtractAssociatedIconW(GetModuleHandleW(nullptr), buf, &index);
	if (icon == nullptr) return out;
	BITMAPINFO bi{};
	bi.bmiHeader.biSize = sizeof bi.bmiHeader;
	bi.bmiHeader.biWidth = px;
	bi.bmiHeader.biHeight = -px;
	bi.bmiHeader.biPlanes = 1;
	bi.bmiHeader.biBitCount = 32;
	bi.bmiHeader.biCompression = BI_RGB;
	HDC screen = GetDC(nullptr);
	HDC dc = CreateCompatibleDC(screen);
	void* bits = nullptr;
	void* mbits = nullptr;
	HBITMAP bmp = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, &bits, nullptr, 0);
	HBITMAP mask = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, &mbits, nullptr, 0);
	ReleaseDC(nullptr, screen);
	if (bmp != nullptr && mask != nullptr && bits != nullptr && mbits != nullptr)
	{
		memset(bits, 0, (size_t)px * px * 4);
		HGDIOBJ old = SelectObject(dc, bmp);
		DrawIconEx(dc, 0, 0, icon, px, px, 0, nullptr, DI_NORMAL);
		SelectObject(dc, mask);
		memset(mbits, 0, (size_t)px * px * 4);
		DrawIconEx(dc, 0, 0, icon, px, px, 0, nullptr, DI_MASK);
		SelectObject(dc, old);
		GdiFlush();
		const unsigned char* p = (const unsigned char*)bits;
		const unsigned char* m = (const unsigned char*)mbits;
		bool anyAlpha = false;
		for (int i = 0; i < px * px; i++)
			if (p[i * 4 + 3] != 0) { anyAlpha = true; break; }
		out.resize((size_t)px * px * 4);
		for (int i = 0; i < px * px; i++)
		{
			unsigned b = p[i * 4], g = p[i * 4 + 1], r = p[i * 4 + 2], a = p[i * 4 + 3];
			if (!anyAlpha) a = m[i * 4] == 0 ? 255 : 0;   // an icon without alpha: the mask says what is drawn
			else if (a > 0 && a < 255)
			{
				// DrawIconEx leaves premultiplied colour: back to straight alpha for the renderer.
				r = r * 255 / a;
				g = g * 255 / a;
				b = b * 255 / a;
				if (r > 255) r = 255;
				if (g > 255) g = 255;
				if (b > 255) b = 255;
			}
			out[(size_t)i * 4] = (unsigned char)r;
			out[(size_t)i * 4 + 1] = (unsigned char)g;
			out[(size_t)i * 4 + 2] = (unsigned char)b;
			out[(size_t)i * 4 + 3] = (unsigned char)a;
		}
	}
	if (bmp) DeleteObject(bmp);
	if (mask) DeleteObject(mask);
	DeleteDC(dc);
	DestroyIcon(icon);
	return out;
}

struct Job {
	std::wstring path;
	int px;
	std::shared_ptr<Item> item;
};

void CALLBACK Work(PTP_CALLBACK_INSTANCE, void* ctx)
{
	std::unique_ptr<Job> job((Job*)ctx);
	HRESULT hr = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
	std::vector<unsigned char> px;
	px = Read(job->path, job->px);
	if (SUCCEEDED(hr)) CoUninitialize();
	{
		std::lock_guard<std::mutex> g(g_lock);
		job->item->rgba = std::move(px);
		job->item->size = job->px;
		job->item->ready = true;
	}
	app::Redraw();
}

ID3D11ShaderResourceView* Upload(const Item& it)
{
	ID3D11Device* dev = app::Device();
	if (dev == nullptr || it.rgba.empty()) return nullptr;
	D3D11_TEXTURE2D_DESC d{};
	d.Width = (UINT)it.size;
	d.Height = (UINT)it.size;
	d.MipLevels = 1;
	d.ArraySize = 1;
	d.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
	d.SampleDesc.Count = 1;
	d.Usage = D3D11_USAGE_DEFAULT;
	d.BindFlags = D3D11_BIND_SHADER_RESOURCE;
	D3D11_SUBRESOURCE_DATA s{};
	s.pSysMem = it.rgba.data();
	s.SysMemPitch = (UINT)it.size * 4;
	ID3D11Texture2D* tex = nullptr;
	if (FAILED(dev->CreateTexture2D(&d, &s, &tex)) || tex == nullptr) return nullptr;
	ID3D11ShaderResourceView* srv = nullptr;
	D3D11_SHADER_RESOURCE_VIEW_DESC v{};
	v.Format = d.Format;
	v.ViewDimension = D3D11_SRV_DIMENSION_TEXTURE2D;
	v.Texture2D.MipLevels = 1;
	dev->CreateShaderResourceView(tex, &v, &srv);
	tex->Release();
	return srv;
}

} // namespace

ImTextureID Get(const std::string& path, int pixels)
{
	if (path.empty()) return (ImTextureID)0;
	std::wstring key = Key(path);
	std::shared_ptr<Item> it;
	{
		std::lock_guard<std::mutex> g(g_lock);
		auto f = g_cache.find(key);
		if (f == g_cache.end())
		{
			it = std::make_shared<Item>();
			g_cache[key] = it;
			auto* job = new Job{ui::Wide(path), pixels > 0 ? pixels : 16, it};
			if (!TrySubmitThreadpoolCallback(Work, job, nullptr))
			{
				delete job;
				it->ready = true;
			}
			return (ImTextureID)0;
		}
		it = f->second;
		if (!it->ready) return (ImTextureID)0;
	}
	if (!it->uploaded)
	{
		it->srv = Upload(*it);
		it->uploaded = true;
		it->rgba.clear();
		it->rgba.shrink_to_fit();
	}
	return (ImTextureID)(intptr_t)it->srv;
}

void Draw(const std::string& path, int pixels, ImVec2 pos, float sizePx)
{
	ImTextureID t = Get(path, pixels);
	if (t == (ImTextureID)0) return;
	ImGui::GetWindowDrawList()->AddImage(ImTextureRef(t), pos, pos + ImVec2(sizePx, sizePx));
}

} // namespace icons

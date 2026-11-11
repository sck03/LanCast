#pragma once
#include <string>
#include <vector>
#include <windows.h>

struct CaptureSource {
    HWND window = nullptr;
    HMONITOR monitor = nullptr;
    DWORD process = 0;
    std::wstring name, display;
    bool available() const;
    bool same_target(const CaptureSource &other) const;
};

// Owns source metadata, never capture resources or UI controls.
class SourceCatalog {
  public:
    int refresh(HWND excluded, int selected);
    const CaptureSource *at(int index) const;
    const std::vector<CaptureSource> &entries() const {
        return entries_;
    }

  private:
    bool initialized_ = false;
    std::vector<CaptureSource> entries_;
};

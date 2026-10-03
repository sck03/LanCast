#pragma once
#include "lancast_media.h"
#include "types.h"
#include <exception>
namespace lancast {
class H264Metadata {
  public:
    explicit H264Metadata(PacketSink output);
    ~H264Metadata();
    void send(const Packet &packet);

  private:
    PacketSink output_;
    std::exception_ptr output_error_;
    HMODULE module_ = nullptr;
    LcH264Filter *filter_ = nullptr;
    decltype(&lc_h264_write) write_ = nullptr;
    decltype(&lc_h264_destroy) destroy_ = nullptr;
};
} // namespace lancast

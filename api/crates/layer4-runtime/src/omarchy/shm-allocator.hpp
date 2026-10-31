// Software-only nested Wayland output. No host device or elevated capability.
#pragma once
#include <aquamarine/allocator/Allocator.hpp>
#include <aquamarine/backend/Backend.hpp>
#include <algorithm>
#include <cmath>
#include <fcntl.h>
#include <sys/mman.h>
#include <unistd.h>

namespace Aquamarine {
    class CSHMBuffer final : public IBuffer {
      public:
        explicit CSHMBuffer(const SAllocatorBufferParams& params) {
            size = params.size;
            if (!std::isfinite(size.x) || !std::isfinite(size.y) || size.x < 1 || size.y < 1 ||
                size.x > 8192 || size.y > 8192 || std::floor(size.x) != size.x || std::floor(size.y) != size.y)
                return;
            format = params.format == DRM_FORMAT_INVALID ? DRM_FORMAT_XRGB8888 : params.format;
            if (format != DRM_FORMAT_XRGB8888 && format != DRM_FORMAT_ARGB8888)
                return;
            stride = static_cast<int>(size.x) * 4;
            length = static_cast<size_t>(stride) * static_cast<size_t>(size.y);
            fd = memfd_create("cma-omarchy-frame", MFD_CLOEXEC | MFD_ALLOW_SEALING);
            if (fd < 0 || ftruncate(fd, static_cast<off_t>(length)) < 0)
                return;
            if (fcntl(fd, F_ADD_SEALS, F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL) < 0)
                return;
            data = mmap(nullptr, length, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
            opaque = format == DRM_FORMAT_XRGB8888;
        }
        ~CSHMBuffer() override {
            events.destroy.emit();
            if (data != MAP_FAILED)
                munmap(data, length);
            if (fd >= 0)
                close(fd);
        }
        eBufferCapability caps() override { return BUFFER_CAPABILITY_DATAPTR; }
        eBufferType type() override { return BUFFER_TYPE_SHM; }
        void update(const Hyprutils::Math::CRegion&) override {}
        bool isSynchronous() override { return true; }
        bool good() override { return data != MAP_FAILED; }
        SSHMAttrs shm() override {
            return {.success = good(), .fd = fd, .format = format, .size = size, .stride = stride, .offset = 0};
        }
        std::tuple<uint8_t*, uint32_t, size_t> beginDataPtr(uint32_t) override {
            return {good() ? static_cast<uint8_t*>(data) : nullptr, format, good() ? length : 0};
        }
        void endDataPtr() override {}
      private:
        int fd = -1, stride = 0;
        uint32_t format = 0;
        size_t length = 0;
        void* data = MAP_FAILED;
    };

    class CSHMAllocator final : public IAllocator {
      public:
        explicit CSHMAllocator(Hyprutils::Memory::CWeakPointer<CBackend> backend) : backend(backend) {}
        Hyprutils::Memory::CSharedPointer<IBuffer> acquire(const SAllocatorBufferParams& params,
                Hyprutils::Memory::CSharedPointer<CSwapchain>) override {
            auto buffer = Hyprutils::Memory::makeShared<CSHMBuffer>(params);
            if (!buffer->good())
                return nullptr;
            return buffer;
        }
        Hyprutils::Memory::CSharedPointer<CBackend> getBackend() override { return backend.lock(); }
        int drmFD() override { return -1; }
        eAllocatorType type() override { return AQ_ALLOCATOR_TYPE_SHM; }
      private:
        Hyprutils::Memory::CWeakPointer<CBackend> backend;
    };
}

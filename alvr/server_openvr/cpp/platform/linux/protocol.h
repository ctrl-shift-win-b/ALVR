#pragma once

#include <array>
#include <atomic>
#include <condition_variable>
#include <cstdint>
#include <cstdlib>
#include <mutex>
#include <vulkan/vulkan.h>

struct present_packet {
    uint32_t image;
    uint32_t frame;
    uint64_t semaphore_value;
    float pose[3][4];
    // CLOCK_MONOTONIC ns when the capture layer finished writing this present.
    // 0 if producer is old / did not stamp. Used for ipc_present_delay profiling.
    uint64_t submit_ns;
};

struct init_packet {
    uint32_t num_images;
    std::array<uint8_t, VK_UUID_SIZE> device_uuid;
    VkImageCreateInfo image_create_info;
    size_t mem_index;
    pid_t source_pid;
};

#include "CEncoder.h"
#include <cmath>

#include <chrono>
#include <cstdlib>
#include <cstring>
#include <errno.h>
#include <exception>
#include <fstream>
#include <iostream>
#include <memory>
#include <poll.h>
#include <sstream>
#include <stdexcept>
#include <stdlib.h>
#include <string>
#include <sys/mman.h>
#include <sys/poll.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include "ALVR-common/packet_types.h"
#include "EncodePipeline.h"
#include "FrameRender.h"
#include "alvr_server/Logger.h"
#include "alvr_server/PoseHistory.h"
#include "alvr_server/Settings.h"
#include "ffmpeg_helper.h"
#include "protocol.h"

extern "C" {
#include <libavutil/avutil.h>
}

CEncoder::CEncoder(std::shared_ptr<PoseHistory> poseHistory)
    : m_poseHistory(poseHistory) { }

CEncoder::~CEncoder() { Stop(); }

namespace {
void read_exactly(pollfd pollfds, char* out, size_t size, std::atomic_bool& exiting) {
    while (not exiting and size != 0) {
        int timeout = 1; // poll api doesn't fit perfectly(100 mircoseconds) poll uses milliseconds
                         // we do the best we can(1000 mircoseconds)
        pollfds.events = POLLIN;
        int count = poll(&pollfds, 1, timeout);
        if (count < 0) {
            throw MakeException("poll failed: %s", strerror(errno));
        } else if (count == 1) {
            int s = read(pollfds.fd, out, size);
            if (s == -1) {
                throw MakeException("read failed: %s", strerror(errno));
            }
            out += s;
            size -= s;
        }
    }
}

void read_latest(pollfd pollfds, char* out, size_t size, std::atomic_bool& exiting) {
    read_exactly(pollfds, out, size, exiting);
    while (not exiting) {
        int timeout = 0; // poll api fixes the original perfectly(0 microseconds)
        pollfds.events = POLLIN;
        int count = poll(&pollfds, 1, timeout);
        if (count == 0)
            return;
        read_exactly(pollfds, out, size, exiting);
    }
}

int accept_timeout(pollfd socket, std::atomic_bool& exiting) {
    while (not exiting) {
        int timeout = 15; // poll api also fits the original perfectly(15000 microseconds)
        socket.events = POLLIN;
        int count = poll(&socket, 1, timeout);
        if (count < 0) {
            throw MakeException("poll failed: %s", strerror(errno));
        } else if (count == 1) {
            return accept4(socket.fd, NULL, NULL, SOCK_CLOEXEC);
        }
    }
    return -1;
}

void av_logfn(void*, int level, const char* data, va_list va) {
    if (level >
#ifdef DEBUG
        AV_LOG_DEBUG)
#else
        AV_LOG_INFO)
#endif
        return;

    char buf[256];
    vsnprintf(buf, sizeof(buf), data, va);

    if (level <= AV_LOG_ERROR)
        Error("Encoder: %s", buf);
    else
        Info("Encoder: %s", buf);
}

} // namespace

void CEncoder::GetFds(int client, int (*received_fds)[6]) {
    struct msghdr msg;
    struct cmsghdr* cmsg;
    union {
        struct cmsghdr cm;
        u_int8_t pktinfo_sizer[sizeof(struct cmsghdr) + 1024];
    } control_un;
    struct iovec iov[1];
    char data[1];
    int ret;

    msg.msg_control = &control_un;
    msg.msg_controllen = sizeof(control_un);
    msg.msg_flags = 0;
    msg.msg_name = NULL;
    msg.msg_namelen = 0;
    iov[0].iov_base = data;
    iov[0].iov_len = 1;
    msg.msg_iov = iov;
    msg.msg_iovlen = 1;

    ret = recvmsg(client, &msg, 0);
    if (ret == -1) {
        throw MakeException("recvmsg failed: %s", strerror(errno));
    }

    for (cmsg = CMSG_FIRSTHDR(&msg); cmsg != NULL; cmsg = CMSG_NXTHDR(&msg, cmsg)) {
        if (cmsg->cmsg_level == SOL_SOCKET && cmsg->cmsg_type == SCM_RIGHTS) {
            memcpy(received_fds, CMSG_DATA(cmsg), sizeof(*received_fds));
            break;
        }
    }

    if (cmsg == NULL) {
        throw MakeException("cmsg is NULL");
    }
}

void CEncoder::Run() {
    // Use Warn so messages hit SteamVR driver log (Info is stats-only and not driver-logged).
    Warn("CEncoder::Run start pid=%d", (int)getpid());
    const char* runtime_dir = getenv("XDG_RUNTIME_DIR");
    if (!runtime_dir || runtime_dir[0] == '\0') {
        Error("CEncoder::Run: XDG_RUNTIME_DIR is unset; cannot create alvr-ipc socket");
        return;
    }
    m_socketPath = runtime_dir;
    m_socketPath += "/alvr-ipc";
    Warn("CEncoder::Run ipc_path=%s", m_socketPath.c_str());

    int ret;
    // we don't really care about what happends with unlink, it's just incase we crashed before this
    // run
    ret = unlink(m_socketPath.c_str());
    if (ret == -1 && errno != ENOENT) {
        Warn("CEncoder::Run: unlink(%s) failed: %s", m_socketPath.c_str(), strerror(errno));
    }

    m_socket.fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    struct sockaddr_un name;
    if (m_socket.fd == -1) {
        Error("CEncoder::Run: socket() failed: %s", strerror(errno));
        perror("socket");
        exit(1);
    }

    memset(&name, 0, sizeof(name));
    name.sun_family = AF_UNIX;
    strncpy(name.sun_path, m_socketPath.c_str(), sizeof(name.sun_path) - 1);

    ret = bind(m_socket.fd, (const struct sockaddr*)&name, sizeof(name));
    if (ret == -1) {
        Error("CEncoder::Run: bind(%s) failed: %s", m_socketPath.c_str(), strerror(errno));
        perror("bind");
        exit(1);
    }

    ret = listen(m_socket.fd, 1024);
    if (ret == -1) {
        Error("CEncoder::Run: listen failed: %s", strerror(errno));
        perror("listen");
        exit(1);
    }

    Warn("CEncoder Listening on %s", m_socketPath.c_str());
    struct pollfd client;
    client.fd = accept_timeout(m_socket, m_exiting);
    if (m_exiting) {
        Warn("CEncoder::Run: exiting before accept");
        return;
    }
    if (client.fd == -1) {
        Error("CEncoder::Run: accept failed or timed out while exiting");
        return;
    }
    init_packet init;
    client.events = POLLIN;
    read_exactly(client, (char*)&init, sizeof(init), m_exiting);
    if (m_exiting)
        return;

    // check that pointer types are null, other values would not make sense over a socket
    assert(init.image_create_info.queueFamilyIndexCount == 0);
    assert(init.image_create_info.pNext == NULL);

    char ifbuf[256];
    char ifbuf2[256];
    sprintf(ifbuf, "/proc/%d/cmdline", (int)init.source_pid);
    std::ifstream ifscmdl(ifbuf);
    ifscmdl >> ifbuf2;
    Warn(
        "CEncoder client connected, pid %d, cmdline %s, num_images=%u",
        (int)init.source_pid,
        ifbuf2,
        (unsigned)init.num_images
    );

    try {
        GetFds(client.fd, &m_fds);

        m_connected = true;

        Warn("CEncoder: initializing Vulkan / FrameRender / EncodePipeline");

        av_log_set_callback(av_logfn);

        alvr::VkContext vk_ctx(init.device_uuid.data(), {});

        FrameRender render(vk_ctx, init, m_fds);
        auto output = render.CreateOutput();

        alvr::VkFrame frame(
            vk_ctx, output.image, output.imageInfo, output.size, output.memory, output.drm
        );
        auto encode_pipeline = alvr::EncodePipeline::Create(
            &render,
            vk_ctx,
            frame,
            output.imageInfo,
            render.GetEncodingWidth(),
            render.GetEncodingHeight()
        );
        Warn(
            "CEncoder: EncodePipeline ready codec=%d size=%ux%u",
            encode_pipeline->GetCodec(),
            (unsigned)render.GetEncodingWidth(),
            (unsigned)render.GetEncodingHeight()
        );

        bool valid_timestamps = true;

        Warn("CEncoder: starting to read present packets");
        present_packet frame_info;
        uint64_t present_count = 0;
        uint64_t pose_miss_count = 0;
        while (not m_exiting) {
            read_latest(client, (char*)&frame_info, sizeof(frame_info), m_exiting);

            encode_pipeline->SetParams(GetDynamicEncoderParams());

            // P2: stamp lag = (1/refresh_rate) * multiplier.
            // Default mult=1.0 → one frame (~11.1ms @ 90Hz). 22ms felt like tiny overshoot.
            // Override: ALVR_LSW_STAMP_FRAME_MULT=0.5|1|1.5|2
            static float s_stamp_mult = []() {
                if (const char* e = std::getenv("ALVR_LSW_STAMP_FRAME_MULT")) {
                    char* end = nullptr;
                    float v = std::strtof(e, &end);
                    if (end != e && v > 0.f && v <= 8.f)
                        return v;
                }
                return 1.0f;
            }();
            const int refresh = Settings::Instance().m_refreshRate > 0
                ? Settings::Instance().m_refreshRate
                : 90;
            const uint64_t one_frame_ns = (uint64_t)(1e9 / (double)refresh);
            uint64_t target_age_ns = (uint64_t)((double)one_frame_ns * (double)s_stamp_mult);
            // Clamp 0.5..4 frames
            if (target_age_ns < one_frame_ns / 2)
                target_age_ns = one_frame_ns / 2;
            if (target_age_ns > one_frame_ns * 4)
                target_age_ns = one_frame_ns * 4;

            auto stamp = m_poseHistory->PickEncodeStamp(
                (const vr::HmdMatrix34_t&)frame_info.pose, target_age_ns
            );
            if (!stamp) {
                pose_miss_count++;
                if (pose_miss_count <= 5 || (pose_miss_count % 500) == 0) {
                    Warn(
                        "CEncoder: pose history miss count=%llu",
                        (unsigned long long)pose_miss_count
                    );
                }
                continue;
            }

            const auto& poseFrame = stamp->frame;
            const auto& match = stamp->matrixMatch;
            const uint64_t pose_ts = poseFrame.targetTimestampNs;

            present_count++;

            if (m_captureFrame) {
                m_captureFrame = false;
                render.CaptureInputFrame(
                    Settings::Instance().m_captureFrameDir + "/alvr_frame_input.ppm"
                );
                render.CaptureOutputFrame(
                    Settings::Instance().m_captureFrameDir + "/alvr_frame_output.ppm"
                );
            }

            render.Render(frame_info.image, frame_info.semaphore_value);

            if (!valid_timestamps) {
                ReportPresent(pose_ts, 0);
                ReportComposed(pose_ts, 0);
            }

            encode_pipeline->PushFrame(pose_ts, m_scheduler.CheckIDRInsertion());

            static_assert(sizeof(frame_info.pose) == sizeof(vr::HmdMatrix34_t&));

            alvr::FramePacket packet;
            if (!encode_pipeline->GetEncoded(packet)) {
                Error("Failed to get encoded data!");
                continue;
            }

            uint64_t present_offset = 0;
            uint64_t composed_offset = 0;
            if (valid_timestamps) {
                auto render_timestamps = render.GetTimestamps();
                auto encode_timestamp = encode_pipeline->GetTimestamp();

                present_offset = render_timestamps.now - render_timestamps.renderBegin;
                composed_offset = 0;

                valid_timestamps = render_timestamps.now != 0;

                if (encode_timestamp.gpu) {
                    composed_offset = render_timestamps.now - encode_timestamp.gpu;
                } else if (encode_timestamp.cpu) {
                    auto now = std::chrono::duration_cast<std::chrono::nanoseconds>(
                                   std::chrono::steady_clock::now().time_since_epoch()
                    )
                                   .count();
                    composed_offset = now - encode_timestamp.cpu;
                } else {
                    composed_offset = render_timestamps.now - render_timestamps.renderComplete;
                }

                if (present_offset < composed_offset) {
                    present_offset = composed_offset;
                }

                ReportPresent(pose_ts, present_offset);
                ReportComposed(pose_ts, composed_offset);
            }

            // MEASURE: matrix vs time_lag vs latest — pick policy is explicit.
            {
                static auto last_log = std::chrono::steady_clock::now();
                static uint64_t log_count = 0;
                static uint64_t last_ts = 0;
                static uint64_t same_ts_run = 0;
                static uint64_t max_same_ts_run = 0;
                static double sum_ts_dt_ms = 0;
                static uint64_t ts_dt_n = 0;
                static uint64_t method_matrix = 0, method_time = 0, method_latest = 0;
                static double sum_rot = 0, sum_pos = 0, sum_age_ms = 0, sum_actual_age_ms = 0;
                static double max_rot = 0, max_age_ms = 0;
                static double sum_comp_vs_stamp_m = 0;
                static double sum_stamp_vs_latest_m = 0;
                static uint64_t hist_size_last = 0;

                log_count++;
                switch (stamp->method) {
                case PoseHistory::StampMethod::MatrixGood:
                    method_matrix++;
                    break;
                case PoseHistory::StampMethod::TimeLag:
                    method_time++;
                    break;
                case PoseHistory::StampMethod::Latest:
                    method_latest++;
                    break;
                }
                sum_rot += match.rotDistanceSq;
                sum_pos += match.posDistanceSq;
                sum_age_ms += (double)match.ageVsLatestNs / 1e6;
                sum_actual_age_ms += (double)stamp->actualAgeNs / 1e6;
                if (match.rotDistanceSq > max_rot)
                    max_rot = match.rotDistanceSq;
                double mage = (double)match.ageVsLatestNs / 1e6;
                if (mage > max_age_ms)
                    max_age_ms = mage;
                hist_size_last = match.historySize;

                {
                    const auto& sp = poseFrame.motion.position;
                    const auto& lp = stamp->latest.motion.position;
                    double dx = match.compositorPos[0] - sp[0];
                    double dy = match.compositorPos[1] - sp[1];
                    double dz = match.compositorPos[2] - sp[2];
                    sum_comp_vs_stamp_m += std::sqrt(dx * dx + dy * dy + dz * dz);
                    dx = sp[0] - lp[0];
                    dy = sp[1] - lp[1];
                    dz = sp[2] - lp[2];
                    sum_stamp_vs_latest_m += std::sqrt(dx * dx + dy * dy + dz * dz);
                }

                if (pose_ts == last_ts) {
                    same_ts_run++;
                    if (same_ts_run > max_same_ts_run)
                        max_same_ts_run = same_ts_run;
                } else {
                    if (last_ts != 0) {
                        sum_ts_dt_ms += (double)(pose_ts - last_ts) / 1e6;
                        ts_dt_n++;
                    }
                    same_ts_run = 1;
                    last_ts = pose_ts;
                }

                auto now = std::chrono::steady_clock::now();
                const double elapsed = std::chrono::duration<double>(now - last_log).count();
                if (elapsed >= 2.0) {
                    double fps = log_count / elapsed;
                    double avg_dt = ts_dt_n ? (sum_ts_dt_ms / ts_dt_n) : 0.0;
                    double n = log_count ? (double)log_count : 1.0;
                    const auto& m = poseFrame.motion;
                    const auto& lm = stamp->latest.motion;
                    Warn(
                        "MEASURE/ENCODE stamp: fps=%.1f n=%llu method matrix/time/latest=%llu/%llu/%llu "
                        "avg_ts_dt=%.2fms max_same_ts_run=%llu pose_misses=%llu hist=%llu "
                        "matrix_rot_dist mean=%.5f max=%.5f matrix_age_ms mean=%.1f max=%.1f "
                        "stamp_age_ms mean=%.1f target_age_ms=%.1f mult=%.2f frame_ms=%.2f "
                        "last_track_ts=%llu "
                        "comp_vs_stamp_m=%.4f stamp_vs_latest_m=%.4f "
                        "stamp_pos=(%.3f,%.3f,%.3f) latest_pos=(%.3f,%.3f,%.3f) "
                        "comp_pos=(%.3f,%.3f,%.3f) present_off_ms=%.2f "
                        "— want: avg_ts_dt~11, max_same<=3, stamp_age~frame_ms, "
                        "comp_pos!=0 when pose_scan OK; method matrix when rot_dist<<1",
                        fps,
                        (unsigned long long)log_count,
                        (unsigned long long)method_matrix,
                        (unsigned long long)method_time,
                        (unsigned long long)method_latest,
                        avg_dt,
                        (unsigned long long)max_same_ts_run,
                        (unsigned long long)pose_miss_count,
                        (unsigned long long)hist_size_last,
                        sum_rot / n,
                        max_rot,
                        sum_age_ms / n,
                        max_age_ms,
                        sum_actual_age_ms / n,
                        (double)target_age_ns / 1e6,
                        s_stamp_mult,
                        1000.0 / (double)refresh,
                        (unsigned long long)m_poseHistory->GetLastTrackingTimestampNs(),
                        sum_comp_vs_stamp_m / n,
                        sum_stamp_vs_latest_m / n,
                        m.position[0],
                        m.position[1],
                        m.position[2],
                        lm.position[0],
                        lm.position[1],
                        lm.position[2],
                        match.compositorPos[0],
                        match.compositorPos[1],
                        match.compositorPos[2],
                        present_offset / 1e6
                    );
                    log_count = 0;
                    max_same_ts_run = 0;
                    sum_ts_dt_ms = 0;
                    ts_dt_n = 0;
                    method_matrix = method_time = method_latest = 0;
                    sum_rot = sum_pos = sum_age_ms = sum_actual_age_ms = 0;
                    max_rot = max_age_ms = 0;
                    sum_comp_vs_stamp_m = sum_stamp_vs_latest_m = 0;
                    last_log = now;
                }
            }

            ParseFrameNals(
                encode_pipeline->GetCodec(), packet.data, packet.size, packet.pts, packet.isIDR
            );
        }
    } catch (std::exception& e) {
        std::stringstream err;
        err << "error in encoder thread: " << e.what();
        Error(err.str().c_str());
    }

    Warn("CEncoder::Run: client loop ended, closing fd");
    client.events = POLLHUP;
    close(client.fd);
}

void CEncoder::Stop() {
    m_exiting = true;
    m_socket.events = POLLHUP;
    close(m_socket.fd);
    unlink(m_socketPath.c_str());
}

void CEncoder::OnStreamStart() { m_scheduler.OnStreamStart(); }

void CEncoder::OnPacketLoss() { m_scheduler.OnPacketLoss(); }

void CEncoder::InsertIDR() { m_scheduler.InsertIDR(); }

void CEncoder::CaptureFrame() { m_captureFrame = true; }

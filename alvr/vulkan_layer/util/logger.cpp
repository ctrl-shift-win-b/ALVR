#include "logger.h"

#include <cstdarg>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <ctime>
#include <mutex>
#include <unistd.h>

namespace {

std::mutex g_log_mutex;

FILE *log_file() {
    static FILE *f = []() -> FILE * {
        FILE *fp = fopen("/tmp/alvr-vulkan-layer.log", "a");
        if (fp) {
            setvbuf(fp, nullptr, _IOLBF, 0);
        }
        return fp;
    }();
    return f;
}

void _log(const char *format, va_list args, bool err) {
    char buf[2048];
    vsnprintf(buf, sizeof(buf), format, args);

    std::lock_guard<std::mutex> lock(g_log_mutex);

    // Always mirror to file for RCA (SteamVR often discards compositor stdout).
    if (FILE *fp = log_file()) {
        time_t now = time(nullptr);
        struct tm tm_now {};
        localtime_r(&now, &tm_now);
        char ts[32];
        strftime(ts, sizeof(ts), "%H:%M:%S", &tm_now);
        fprintf(fp, "[%s pid=%d] %s", ts, (int)getpid(), buf);
        if (buf[0] == '\0' || buf[strlen(buf) - 1] != '\n') {
            fputc('\n', fp);
        }
        fflush(fp);
    }

    fputs(buf, err ? stderr : stdout);
    if (buf[0] == '\0' || buf[strlen(buf) - 1] != '\n') {
        fputc('\n', err ? stderr : stdout);
    }
}

} // namespace

void Error(const char *format, ...) {
    va_list args;
    va_start(args, format);
    _log(format, args, true);
    va_end(args);
}

void Warn(const char *format, ...) {
    va_list args;
    va_start(args, format);
    _log(format, args, true);
    va_end(args);
}

void Info(const char *format, ...) {
    va_list args;
    va_start(args, format);
    _log(format, args, false);
    va_end(args);
}

void Debug(const char *format, ...) {
    // Always file-log Debug during RCA; still gate stderr unless ALVR_LOG_DEBUG is set.
    va_list args;
    va_start(args, format);
    char buf[2048];
    vsnprintf(buf, sizeof(buf), format, args);
    va_end(args);

    {
        std::lock_guard<std::mutex> lock(g_log_mutex);
        if (FILE *fp = log_file()) {
            time_t now = time(nullptr);
            struct tm tm_now {};
            localtime_r(&now, &tm_now);
            char ts[32];
            strftime(ts, sizeof(ts), "%H:%M:%S", &tm_now);
            fprintf(fp, "[%s pid=%d DEBUG] %s", ts, (int)getpid(), buf);
            if (buf[0] == '\0' || buf[strlen(buf) - 1] != '\n') {
                fputc('\n', fp);
            }
            fflush(fp);
        }
    }

    if (getenv("ALVR_LOG_DEBUG") != nullptr) {
        fputs(buf, stderr);
        if (buf[0] == '\0' || buf[strlen(buf) - 1] != '\n') {
            fputc('\n', stderr);
        }
    }
}

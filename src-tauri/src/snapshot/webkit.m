#import <AppKit/AppKit.h>
#import <WebKit/WebKit.h>
#if __MAC_OS_X_VERSION_MAX_ALLOWED >= 260000
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#endif

typedef void (*Completion)(void *, const char *);

static const char *message(NSError *error) {
    NSString *description =
        error.userInfo[@"WKJavaScriptExceptionMessage"] ?: error.localizedDescription;
    return description.UTF8String;
}

void fw_snapshot_script(void *view, const char *input, void *context, Completion done) {
    WKWebView *webview = (__bridge WKWebView *)view;
    NSString *body = [NSString stringWithUTF8String:input];
    if (@available(macOS 11, *)) {
        [webview callAsyncJavaScript:body
            arguments:@{}
            inFrame:nil
            inContentWorld:WKContentWorld.pageWorld
            completionHandler:^(id result, NSError *error) {
                (void)result;
                done(context, message(error));
            }];
    } else {
        done(context, "Snapshot mode requires macOS 11 or later.");
    }
}

static void write_image(NSImage *image, NSString *path, NSError *error, void *context, Completion done) {
    if (error) {
        done(context, message(error));
        return;
    }
    NSBitmapImageRep *bitmap = [NSBitmapImageRep imageRepWithData:image.TIFFRepresentation];
    NSData *png = [bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
    if (!png) {
        done(context, "WebKit returned an empty snapshot.");
        return;
    }
    [png writeToFile:path options:NSDataWritingAtomic error:&error];
    done(context, message(error));
}

void fw_snapshot_capture(void *view, const char *input, void *context, Completion done) {
    WKWebView *webview = (__bridge WKWebView *)view;
    NSString *path = [NSString stringWithUTF8String:input];
    WKSnapshotConfiguration *options = [WKSnapshotConfiguration new];
    options.afterScreenUpdates = YES;
    // Cocoa title-bar rounding can enlarge the content view by a point at Retina scale.
    NSString *size = NSProcessInfo.processInfo.environment[@"APP_SNAPSHOT_SIZE"];
    if (size) {
        NSArray<NSString *> *parts = [size.lowercaseString componentsSeparatedByString:@"x"];
        NSRect bounds = webview.bounds;
        options.rect =
            NSMakeRect(bounds.origin.x, bounds.origin.y, parts[0].doubleValue, parts[1].doubleValue);
    }
    [webview takeSnapshotWithConfiguration:options
                         completionHandler:^(NSImage *image, NSError *error) {
                             write_image(image, path, error, context, done);
                         }];
}

void fw_snapshot_about(void *view, const char *input, void *context, Completion done) {
    WKWebView *webview = (__bridge WKWebView *)view;
    NSApp.appearance = webview.window.effectiveAppearance;
    [NSApp orderFrontStandardAboutPanelWithOptions:@{}];
    NSWindow *panel = nil;
    for (NSWindow *candidate in NSApp.windows) {
        if (candidate != webview.window && candidate.isVisible && candidate.contentView) {
            panel = candidate;
            break;
        }
    }
    if (!panel) {
        done(context, "AppKit could not open its About panel.");
        return;
    }
    [panel orderOut:nil];
    [panel displayIfNeeded];
    NSView *content = panel.contentView.superview ?: panel.contentView;
    NSBitmapImageRep *bitmap = [content bitmapImageRepForCachingDisplayInRect:content.bounds];
    if (!bitmap) {
        done(context, "AppKit returned an empty About panel.");
        return;
    }
    [content cacheDisplayInRect:content.bounds toBitmapImageRep:bitmap];
    NSData *data = [bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
    if (!data) {
        done(context, "Could not encode the About panel.");
        return;
    }
    NSError *error = nil;
    [data writeToFile:[NSString stringWithUTF8String:input] options:NSDataWritingAtomic error:&error];
    done(context, message(error));
}

void fw_snapshot_present(void *view, const char *input, void *context, Completion done) {
    (void)input;
    WKWebView *webview = (__bridge WKWebView *)view;
    [webview.window orderBack:nil];
    done(context, NULL);
}

void fw_snapshot_window(void *view, const char *input, void *context, Completion done) {
    WKWebView *webview = (__bridge WKWebView *)view;
// WebKit content snapshots exclude sibling native layers. ScreenCaptureKit can
// capture this process's own compositor surface without reading other windows.
#if __MAC_OS_X_VERSION_MAX_ALLOWED >= 260000
    if (@available(macOS 26, *)) {
        const CGWindowID wid = (CGWindowID)webview.window.windowNumber;
        NSString *path = [NSString stringWithUTF8String:input];
        [SCShareableContent getCurrentProcessShareableContentWithCompletionHandler:^(
                                SCShareableContent *content, NSError *error) {
            if (error) {
                done(context, message(error));
                return;
            }
            SCWindow *own = nil;
            for (SCWindow *candidate in content.windows)
                if (candidate.windowID == wid) {
                    own = candidate;
                    break;
                }
            if (!own) {
                done(context, "The snapshot window is unavailable to the macOS compositor.");
                return;
            }
            SCContentFilter *filter = [[SCContentFilter alloc] initWithDesktopIndependentWindow:own];
            SCStreamConfiguration *config = [SCStreamConfiguration new];
            config.width = (size_t)round(filter.contentRect.size.width * filter.pointPixelScale);
            config.height = (size_t)round(filter.contentRect.size.height * filter.pointPixelScale);
            config.showsCursor = NO;
            config.ignoreShadowsSingleWindow = YES;
            [SCScreenshotManager
                captureImageWithFilter:filter
                         configuration:config
                     completionHandler:^(CGImageRef image, NSError *captureError) {
                         if (captureError || !image) {
                             done(context,
                                  captureError ? message(captureError) : "Empty window snapshot.");
                             return;
                         }
                         NSBitmapImageRep *bitmap = [[NSBitmapImageRep alloc] initWithCGImage:image];
                         NSData *png = [bitmap representationUsingType:NSBitmapImageFileTypePNG
                                                            properties:@{}];
                         NSError *writeError = nil;
                         if (!png) {
                             done(context, "Could not encode the window snapshot.");
                             return;
                         }
                         [png writeToFile:path options:NSDataWritingAtomic error:&writeError];
                         done(context, message(writeError));
                     }];
        }];
        return;
    }
#else
    (void)webview;
    (void)input;
#endif
    done(context, "Glass compositor snapshots require the macOS 26 SDK and runtime.");
}

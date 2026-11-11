#import <AppKit/AppKit.h>
#import <CoreImage/CoreImage.h>
#import <WebKit/WebKit.h>
#import <objc/runtime.h>

typedef struct {
    double x, y, width, height, radius;
} FWRegion;

#if __MAC_OS_X_VERSION_MAX_ALLOWED >= 260000
// Navigation remains in WebKit's accessibility and event trees.
@interface FWGlassBackdrop : NSView
@end
@implementation FWGlassBackdrop
- (BOOL)wantsUpdateLayer {
    return YES;
}
- (void)updateLayer {
    // Let AppKit resolve a semantic canvas in the window's current appearance.
    // The glass samples this partially transparent layer and its actual backdrop.
    self.layer.backgroundColor = [NSColor.controlBackgroundColor colorWithAlphaComponent:0.64].CGColor;
}
- (void)viewDidChangeEffectiveAppearance {
    [super viewDidChangeEffectiveAppearance];
    self.needsDisplay = YES;
}
- (NSView *)hitTest:(NSPoint)point {
    (void)point;
    return nil;
}
- (BOOL)isAccessibilityElement {
    return NO;
}
@end

@interface FWMaterialState : NSObject
@property(strong) FWGlassBackdrop *backdrop;
@property(strong) NSArray<CIFilter *> *previousFilters;
@property(strong) NSColor *windowColor;
@property(strong) NSColor *pageColor;
@property BOOL opaque;
@property BOOL layerUsedCoreImage;
@property(strong) NSData *geometry;
@property NSSize viewport;
@property CGFloat pageInset;
@end
@implementation FWMaterialState
@end

static char stateKey;

// Neutral pixels pass through the neutral table exactly. In the keyed table,
// remove the magenta contribution from premultiplied antialiased glyph edges.
static NSArray<NSData *> *tables(void) {
    static NSArray<NSData *> *result;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        const NSUInteger side = 32, count = side * side * side * 4;
        NSMutableData *baseData = [NSMutableData dataWithLength:count * sizeof(float)];
        NSMutableData *keyData = [NSMutableData dataWithLength:count * sizeof(float)];
        float *base = baseData.mutableBytes, *key = keyData.mutableBytes;
        for (NSUInteger b = 0; b < side; b++)
            for (NSUInteger g = 0; g < side; g++)
                for (NSUInteger r = 0; r < side; r++) {
                    float red = (float)r / (side - 1), green = (float)g / (side - 1),
                          blue = (float)b / (side - 1);
                    float magenta = fmaxf(0, fminf(red, blue) - green);
                    float balance = fmaxf(0, 1 - fabsf(red - blue) * 4);
                    magenta *= balance;
                    const NSUInteger i = 4 * (r + side * (g + side * b));
                    // During live resize the DOM clip and native geometry may arrive a frame
                    // apart. Only the one reserved saturated corner is keyed outside the regions.
                    float neutralAlpha = (r == side - 1 && g == 0 && b == side - 1) ? 0 : 1;
                    base[i] = red * neutralAlpha;
                    base[i + 1] = green * neutralAlpha;
                    base[i + 2] = blue * neutralAlpha;
                    base[i + 3] = neutralAlpha;
                    key[i] = red - magenta;
                    key[i + 1] = green;
                    key[i + 2] = blue - magenta;
                    key[i + 3] = 1 - magenta;
                }
        result = @[ baseData, keyData ];
    });
    return result;
}

static CIImage *maskForRegions(NSSize size, CGFloat pageInset, const FWRegion *regions, size_t count) {
    const size_t w = MAX(1, (size_t)ceil(size.width)), h = MAX(1, (size_t)ceil(size.height));
    CGColorSpaceRef gray = CGColorSpaceCreateDeviceGray();
    CGContextRef ctx = CGBitmapContextCreate(NULL, w, h, 8, 0, gray, (CGBitmapInfo)kCGImageAlphaNone);
    CGColorSpaceRelease(gray);
    if (!ctx)
        return nil;
    CGContextSetGrayFillColor(ctx, 1, 1);
    for (size_t i = 0; i < count; i++) {
        FWRegion r = regions[i];
        // AppKit passes the WK layer to the Core Image filter in flipped view coordinates.
        // Allow one device-independent pixel of CSS antialiasing at the border.
        // Only keyed pixels in this rectangle qualify for the transparent material.
        CGRect box = CGRectInset(CGRectMake(r.x, pageInset + r.y, r.width, r.height), -1, -1);
        CGContextFillRect(ctx, box);
    }
    CGImageRef image = CGBitmapContextCreateImage(ctx);
    CGContextRelease(ctx);
    if (!image)
        return nil;
    CIImage *mask = [CIImage imageWithCGImage:image];
    CGImageRelease(image);
    return mask;
}

API_AVAILABLE(macos(26.0))
static void uninstall(WKWebView *web, FWMaterialState *state) {
    if (!state)
        return;
    web.contentFilters = state.previousFilters;
    web.layerUsesCoreImageFilters = state.layerUsedCoreImage;
    web.underPageBackgroundColor = state.pageColor;
    web.window.backgroundColor = state.windowColor;
    web.window.opaque = state.opaque;
    [state.backdrop removeFromSuperview];
    objc_setAssociatedObject(web, &stateKey, nil, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
}

API_AVAILABLE(macos(26.0))
static BOOL setMaterial(WKWebView *web, BOOL enabled, const FWRegion *regions, size_t count) {
    FWMaterialState *state = objc_getAssociatedObject(web, &stateKey);
    if (!enabled || !count) {
        uninstall(web, state);
        return NO;
    }
    NSView *parent = web.superview;
    if (!parent || !web.window) {
        uninstall(web, state);
        return NO;
    }
    NSData *geometry = [NSData dataWithBytes:regions length:count * sizeof(FWRegion)];
    NSSize size = web.bounds.size;
    // Tauri lays WKWebView beneath the native title bar. WebKit viewport CSS
    // coordinates start at the content layout guide, below that bar.
    NSRect layout = [web convertRect:web.window.contentLayoutRect fromView:nil];
    CGFloat pageInset = MAX(0, web.isFlipped ? NSMinY(layout) - NSMinY(web.bounds)
                                             : NSMaxY(web.bounds) - NSMaxY(layout));
    if (state && NSEqualSizes(state.viewport, size) && state.pageInset == pageInset &&
        [geometry isEqualToData:state.geometry])
        return YES;
    CIImage *mask = maskForRegions(size, pageInset, regions, count);
    if (!mask) {
        uninstall(web, state);
        return NO;
    }
    NSArray<NSData *> *cube = tables();
    CIFilter *filter = [CIFilter filterWithName:@"CIColorCubesMixedWithMask"
                            withInputParameters:@{
                                @"inputCubeDimension" : @32,
                                @"inputCube0Data" : cube[0],
                                @"inputCube1Data" : cube[1],
                                @"inputMaskImage" : mask
                            }];
    if (!filter) {
        uninstall(web, state);
        return NO;
    }
    if (!state) {
        state = [FWMaterialState new];
        state.previousFilters = web.contentFilters ?: @[];
        state.pageColor = web.underPageBackgroundColor;
        state.windowColor = web.window.backgroundColor;
        state.opaque = web.window.opaque;
        state.layerUsedCoreImage = web.layerUsesCoreImageFilters;
        state.backdrop = [[FWGlassBackdrop alloc] initWithFrame:web.frame];
        state.backdrop.wantsLayer = YES;
        state.backdrop.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
        [parent addSubview:state.backdrop positioned:NSWindowBelow relativeTo:web];
        objc_setAssociatedObject(web, &stateKey, state, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
        web.window.opaque = NO;
        web.window.backgroundColor = NSColor.clearColor;
        web.underPageBackgroundColor = NSColor.clearColor;
        web.wantsLayer = YES;
        web.layerUsesCoreImageFilters = YES;
    }
    state.viewport = size;
    state.pageInset = pageInset;
    state.geometry = geometry;
    state.backdrop.frame = web.frame;
    while (state.backdrop.subviews.count > count)
        [state.backdrop.subviews.lastObject removeFromSuperview];
    for (size_t i = 0; i < count; i++) {
        FWRegion r = regions[i];
        NSRect frame = NSMakeRect(r.x, size.height - pageInset - r.y - r.height, r.width, r.height);
        NSGlassEffectView *glass = nil;
        if (i < state.backdrop.subviews.count)
            glass = (NSGlassEffectView *)state.backdrop.subviews[i];
        else {
            glass = [[NSGlassEffectView alloc] initWithFrame:frame];
            glass.style = NSGlassEffectViewStyleRegular;
            [state.backdrop addSubview:glass];
        }
        glass.frame = frame;
        glass.cornerRadius = MIN(r.radius, MIN(r.width, r.height) * 0.5);
    }
    web.contentFilters = [state.previousFilters arrayByAddingObject:filter];
    return YES;
}
#endif

unsigned fw_glass_status(void *view) {
    const BOOL reduced = NSWorkspace.sharedWorkspace.accessibilityDisplayShouldReduceTransparency;
    unsigned result = reduced ? 2 : 0;
#if __MAC_OS_X_VERSION_MAX_ALLOWED >= 260000
    if (@available(macOS 26, *)) {
        result |= 1;
        if (!reduced && objc_getAssociatedObject((__bridge WKWebView *)view, &stateKey))
            result |= 4;
    }
#else
    (void)view;
#endif
    return result;
}

unsigned fw_glass_set(void *view, bool enabled, const FWRegion *regions, size_t count) {
#if __MAC_OS_X_VERSION_MAX_ALLOWED >= 260000
    if (@available(macOS 26, *)) {
        setMaterial((__bridge WKWebView *)view,
                    enabled && !NSWorkspace.sharedWorkspace.accessibilityDisplayShouldReduceTransparency,
                    regions, count);
    }
#else
    (void)enabled;
    (void)regions;
    (void)count;
#endif
    return fw_glass_status(view);
}

void fw_glass_observe(void (*changed)(void)) {
    static id token;
    if (!token)
        token = [NSWorkspace.sharedWorkspace.notificationCenter
            addObserverForName:NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification
                        object:nil
                         queue:NSOperationQueue.mainQueue
                    usingBlock:^(NSNotification *note) {
                        (void)note;
                        changed();
                    }];
}

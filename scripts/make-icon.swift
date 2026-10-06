// Draws the Better Finder app icon and writes assets/AppIcon.icns.
//
// Run from any directory: swift scripts/make-icon.swift
//
// The icon follows the macOS icon grid: a 1024 px canvas, and a squircle of
// 824 px with continuous corners at the center. The script draws all shapes
// itself. (The SF Symbols license does not allow symbols in app icons.)

import AppKit

let repo = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
let assets = repo.appendingPathComponent("assets")
let iconset = assets.appendingPathComponent("AppIcon.iconset")

func rgb(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(
        srgbRed: CGFloat((hex >> 16) & 0xff) / 255,
        green: CGFloat((hex >> 8) & 0xff) / 255,
        blue: CGFloat(hex & 0xff) / 255,
        alpha: alpha
    )
}

/// A rounded rectangle with continuous corners (the Apple squircle).
/// The control points are the known values for the iOS and macOS corner curve.
func continuousRect(_ rect: CGRect, radius r: CGFloat) -> CGPath {
    let path = CGMutablePath()
    // Each corner: the corner point, the unit vector back along the edge
    // that comes in, and the unit vector along the edge that goes out.
    let corners: [(CGPoint, CGVector, CGVector)] = [
        (CGPoint(x: rect.maxX, y: rect.minY), CGVector(dx: -1, dy: 0), CGVector(dx: 0, dy: 1)),
        (CGPoint(x: rect.maxX, y: rect.maxY), CGVector(dx: 0, dy: -1), CGVector(dx: -1, dy: 0)),
        (CGPoint(x: rect.minX, y: rect.maxY), CGVector(dx: 1, dy: 0), CGVector(dx: 0, dy: -1)),
        (CGPoint(x: rect.minX, y: rect.minY), CGVector(dx: 0, dy: 1), CGVector(dx: 1, dy: 0)),
    ]
    func point(_ c: (CGPoint, CGVector, CGVector), _ x: CGFloat, _ y: CGFloat) -> CGPoint {
        CGPoint(x: c.0.x + (c.1.dx * x + c.2.dx * y) * r, y: c.0.y + (c.1.dy * x + c.2.dy * y) * r)
    }
    path.move(to: point(corners[3], 0, 1.528665))
    for c in corners {
        path.addLine(to: point(c, 1.528665, 0))
        path.addCurve(to: point(c, 0.631493, 0.074911), control1: point(c, 1.088492, 0), control2: point(c, 0.868406, 0))
        path.addCurve(to: point(c, 0.074911, 0.631493), control1: point(c, 0.372824, 0.168478), control2: point(c, 0.168478, 0.372824))
        path.addCurve(to: point(c, 0, 1.528665), control1: point(c, 0, 0.868406), control2: point(c, 0, 1.088492))
    }
    path.closeSubpath()
    return path
}

func fillGradient(_ cg: CGContext, _ path: CGPath, _ top: CGColor, _ bottom: CGColor) {
    let box = path.boundingBox
    cg.saveGState()
    cg.addPath(path)
    cg.clip()
    let gradient = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB), colors: [top, bottom] as CFArray, locations: [0, 1])!
    cg.drawLinearGradient(gradient, start: CGPoint(x: box.midX, y: box.maxY), end: CGPoint(x: box.midX, y: box.minY), options: [])
    cg.restoreGState()
}

/// Draws the icon in a 1024 x 1024 space (origin at the bottom left).
func drawIcon(_ cg: CGContext) {
    let body = continuousRect(CGRect(x: 100, y: 100, width: 824, height: 824), radius: 185.4)

    // The drop shadow of the icon body, from the macOS template.
    cg.saveGState()
    cg.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: rgb(0x000000, 0.35))
    cg.addPath(body)
    cg.setFillColor(rgb(0x2448d8))
    cg.fillPath()
    cg.restoreGState()

    // The body: a vivid blue gradient.
    fillGradient(cg, body, rgb(0x4f8dff), rgb(0x1b3cc4))

    // The folder: the back panel with its tab, then the front panel.
    let back = CGMutablePath()
    back.addPath(CGPath(roundedRect: CGRect(x: 232, y: 300, width: 560, height: 372), cornerWidth: 44, cornerHeight: 44, transform: nil))
    back.addPath(CGPath(roundedRect: CGRect(x: 232, y: 600, width: 236, height: 112), cornerWidth: 36, cornerHeight: 36, transform: nil))
    fillGradient(cg, back, rgb(0xd6e4ff), rgb(0xa9c5ff))

    let front = CGPath(roundedRect: CGRect(x: 232, y: 280, width: 560, height: 344), cornerWidth: 44, cornerHeight: 44, transform: nil)
    cg.saveGState()
    cg.setShadow(offset: CGSize(width: 0, height: -6), blur: 18, color: rgb(0x0b1f73, 0.35))
    cg.addPath(front)
    cg.setFillColor(rgb(0xffffff))
    cg.fillPath()
    cg.restoreGState()
    fillGradient(cg, front, rgb(0xffffff), rgb(0xe3ecff))

    // The terminal prompt on the folder: a chevron and a cursor bar.
    cg.saveGState()
    cg.setLineWidth(50)
    cg.setLineCap(.round)
    cg.setLineJoin(.round)
    cg.setStrokeColor(rgb(0x1f43cf))
    cg.move(to: CGPoint(x: 392, y: 528))
    cg.addLine(to: CGPoint(x: 478, y: 452))
    cg.addLine(to: CGPoint(x: 392, y: 376))
    cg.strokePath()
    cg.restoreGState()
    let cursor = CGPath(roundedRect: CGRect(x: 528, y: 352, width: 128, height: 48), cornerWidth: 24, cornerHeight: 24, transform: nil)
    cg.addPath(cursor)
    cg.setFillColor(rgb(0x3d74ff))
    cg.fillPath()

    // A glass highlight on the top part of the body, and a thin light edge.
    cg.saveGState()
    cg.addPath(body)
    cg.clip()
    let gloss = CGGradient(
        colorsSpace: CGColorSpace(name: CGColorSpace.sRGB),
        colors: [rgb(0xffffff, 0.28), rgb(0xffffff, 0.0)] as CFArray,
        locations: [0, 1]
    )!
    cg.drawLinearGradient(gloss, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 560), options: [])
    cg.restoreGState()

    cg.saveGState()
    cg.addPath(body)
    cg.clip()
    cg.addPath(body)
    cg.setLineWidth(6)
    cg.setStrokeColor(rgb(0xffffff, 0.22))
    cg.strokePath()
    cg.restoreGState()
}

func render(_ pixels: Int) -> Data {
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .calibratedRGB, bytesPerRow: 0, bitsPerPixel: 0
    )!
    let context = NSGraphicsContext(bitmapImageRep: rep)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = context
    let cg = context.cgContext
    cg.setShouldAntialias(true)
    cg.interpolationQuality = .high
    let scale = CGFloat(pixels) / 1024
    cg.scaleBy(x: scale, y: scale)
    drawIcon(cg)
    NSGraphicsContext.restoreGraphicsState()
    return rep.retagging(with: .sRGB)!.representation(using: .png, properties: [:])!
}

let fm = FileManager.default
try? fm.removeItem(at: iconset)
try fm.createDirectory(at: iconset, withIntermediateDirectories: true)

// The names and pixel sizes that iconutil expects.
let sizes: [(String, Int)] = [
    ("icon_16x16.png", 16), ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32), ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128), ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256), ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512), ("icon_512x512@2x.png", 1024),
]
for (name, pixels) in sizes {
    try render(pixels).write(to: iconset.appendingPathComponent(name))
}
try render(1024).write(to: assets.appendingPathComponent("AppIcon-1024.png"))

let iconutil = Process()
iconutil.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
iconutil.arguments = ["-c", "icns", iconset.path, "-o", assets.appendingPathComponent("AppIcon.icns").path]
try iconutil.run()
iconutil.waitUntilExit()
try? fm.removeItem(at: iconset)
guard iconutil.terminationStatus == 0 else {
    FileHandle.standardError.write("iconutil failed\n".data(using: .utf8)!)
    exit(1)
}
print("Wrote assets/AppIcon.icns and assets/AppIcon-1024.png")

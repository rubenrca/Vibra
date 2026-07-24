#!/usr/bin/env swift

import AppKit
import Foundation

guard CommandLine.arguments.count == 3 else {
    FileHandle.standardError.write(
        Data("usage: generate_app_icon.swift <source.png> <output.iconset>\n".utf8)
    )
    exit(64)
}

let sourceURL = URL(fileURLWithPath: CommandLine.arguments[1])
let outputURL = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)
let fileManager = FileManager.default

guard let sourceImage = NSImage(contentsOf: sourceURL) else {
    FileHandle.standardError.write(Data("Unable to read \(sourceURL.path)\n".utf8))
    exit(66)
}

if fileManager.fileExists(atPath: outputURL.path) {
    try fileManager.removeItem(at: outputURL)
}
try fileManager.createDirectory(at: outputURL, withIntermediateDirectories: true)

/// macOS lays every Dock icon out on the same grid: the rounded body covers
/// 824 pt of a 1024 pt canvas, leaving a 100 pt margin, and casts a soft
/// shadow biased downwards. Measured against Terminal.icns and Notes.icns,
/// whose bodies are exactly 824x824 and whose shadows reach 87 pt above and
/// 72 pt below the canvas edge. Drawing full-bleed instead makes the icon
/// render ~24% larger than every neighbour.
let canvasReference: CGFloat = 1024
let bodyFraction: CGFloat = 824 / canvasReference
let cornerFraction: CGFloat = 185.4 / 824
let shadowBlurFraction: CGFloat = 20 / canvasReference
let shadowOffsetFraction: CGFloat = 7.5 / canvasReference

let variants: [(name: String, pixels: Int)] = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
]

for variant in variants {
    guard let bitmap = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: variant.pixels,
        pixelsHigh: variant.pixels,
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bytesPerRow: 0,
        bitsPerPixel: 0
    ), let context = NSGraphicsContext(bitmapImageRep: bitmap) else {
        throw IconGenerationError.bitmapCreationFailed(variant.pixels)
    }

    let size = CGFloat(variant.pixels)
    let canvas = NSRect(x: 0, y: 0, width: size, height: size)
    let body = canvas.insetBy(
        dx: size * (1 - bodyFraction) / 2,
        dy: size * (1 - bodyFraction) / 2
    )
    let corner = body.width * cornerFraction
    let source = NSRect(origin: .zero, size: sourceImage.size)

    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = context
    context.imageInterpolation = .high
    NSColor.clear.setFill()
    canvas.fill()

    // Cast the shadow from an opaque stand-in first: clipping the artwork and
    // shadowing it in one pass would blur the shadow through the artwork's own
    // alpha instead of around the body's silhouette.
    NSGraphicsContext.saveGraphicsState()
    let shadow = NSShadow()
    shadow.shadowColor = NSColor.black.withAlphaComponent(0.32)
    shadow.shadowBlurRadius = size * shadowBlurFraction
    shadow.shadowOffset = NSSize(width: 0, height: -size * shadowOffsetFraction)
    shadow.set()
    NSColor.black.setFill()
    NSBezierPath(roundedRect: body, xRadius: corner, yRadius: corner).fill()
    NSGraphicsContext.restoreGraphicsState()

    NSBezierPath(roundedRect: body, xRadius: corner, yRadius: corner).addClip()
    sourceImage.draw(
        in: body,
        from: source,
        operation: .sourceOver,
        fraction: 1
    )
    context.flushGraphics()
    NSGraphicsContext.restoreGraphicsState()

    guard let data = bitmap.representation(using: .png, properties: [:]) else {
        throw IconGenerationError.pngEncodingFailed(variant.pixels)
    }
    try data.write(to: outputURL.appendingPathComponent(variant.name), options: .atomic)
}

private enum IconGenerationError: LocalizedError {
    case bitmapCreationFailed(Int)
    case pngEncodingFailed(Int)

    var errorDescription: String? {
        switch self {
        case .bitmapCreationFailed(let size):
            "Unable to create the \(size) px icon bitmap."
        case .pngEncodingFailed(let size):
            "Unable to encode the \(size) px icon as PNG."
        }
    }
}

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
    let destination = NSRect(x: 0, y: 0, width: size, height: size)
    let source = NSRect(origin: .zero, size: sourceImage.size)

    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = context
    context.imageInterpolation = .high
    NSColor.clear.setFill()
    destination.fill()
    NSBezierPath(
        roundedRect: destination,
        xRadius: size * 0.2237,
        yRadius: size * 0.2237
    ).addClip()
    sourceImage.draw(
        in: destination,
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

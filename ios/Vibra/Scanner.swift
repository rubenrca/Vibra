import AVFoundation
import SwiftUI

struct Scanner: UIViewControllerRepresentable {
  let scanned: (String) -> Void
  func makeUIViewController(context: Context) -> ScannerController {
    let controller = ScannerController()
    controller.scanned = scanned
    return controller
  }
  func updateUIViewController(_ controller: ScannerController, context: Context) {}
  static func dismantleUIViewController(_ controller: ScannerController, coordinator: ()) {
    controller.stop()
  }
}
final class ScannerController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
  var scanned: ((String) -> Void)?
  private let session = AVCaptureSession()
  private let worker = DispatchQueue(label: "app.vibra.camera")
  private var preview: AVCaptureVideoPreviewLayer?
  private var consumed = false
  private var stopped = false
  override func viewDidLoad() {
    super.viewDidLoad()
    view.backgroundColor = .black
    AVCaptureDevice.requestAccess(for: .video) { [weak self] allowed in
      DispatchQueue.main.async {
        guard let self, !self.stopped else { return }
        if allowed {
          self.start()
        } else {
          self.explain("Permite el acceso a la cámara en Ajustes para escanear el QR.")
        }
      }
    }
  }
  private func start() {
    guard let camera = AVCaptureDevice.default(for: .video),
      let input = try? AVCaptureDeviceInput(device: camera), session.canAddInput(input)
    else {
      explain("Cámara no disponible. Puedes pegar la invitación en la pantalla anterior.")
      return
    }
    session.addInput(input)
    let output = AVCaptureMetadataOutput()
    guard session.canAddOutput(output) else { return }
    session.addOutput(output)
    output.setMetadataObjectsDelegate(self, queue: .main)
    output.metadataObjectTypes = [.qr]
    let preview = AVCaptureVideoPreviewLayer(session: session)
    preview.videoGravity = .resizeAspectFill
    view.layer.addSublayer(preview)
    self.preview = preview
    preview.frame = view.bounds
    worker.async { self.session.startRunning() }
  }
  override func viewDidLayoutSubviews() {
    super.viewDidLayoutSubviews()
    preview?.frame = view.bounds
  }
  func metadataOutput(
    _ output: AVCaptureMetadataOutput, didOutput objects: [AVMetadataObject],
    from connection: AVCaptureConnection
  ) {
    guard !consumed,
      let value = (objects.first as? AVMetadataMachineReadableCodeObject)?.stringValue
    else { return }
    consumed = true
    stop()
    scanned?(value)
  }
  func stop() {
    stopped = true
    worker.async { self.session.stopRunning() }
  }
  private func explain(_ text: String) {
    let label = UILabel(frame: view.bounds.insetBy(dx: 24, dy: 24))
    label.text = text
    label.textColor = .white
    label.numberOfLines = 0
    label.textAlignment = .center
    label.autoresizingMask = [.flexibleWidth, .flexibleHeight]
    view.addSubview(label)
  }
}

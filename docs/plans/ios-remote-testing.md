# Probar Vibra Remote

Vibra Mac y Vibra Remote para iPhone se conectan dentro de la misma red local, sin servidores externos. Requiere macOS 14 o posterior e iOS 17 o posterior.

## Vincular y compartir

1. Abre Vibra en el Mac y entra en Ajustes → iPhone → Vincular iPhone.
2. Escanea el QR desde Vibra Remote y acepta la solicitud en el Mac. También puedes copiar y pegar la invitación; caduca a los cinco minutos y solo se puede usar una vez.
3. Haz clic derecho dentro de una terminal del Mac y elige Compartir con iPhone.
4. Selecciona esa terminal en el iPhone. Al cerrarla, desconectarte o pasar la app a segundo plano, el Mac recupera el control.

El Mac recuerda la vinculación, pero el acceso remoto empieza desactivado tras reiniciar Vibra. Actívalo en Ajustes y vuelve a compartir las terminales que quieras controlar. Desvincular desde el Mac revoca el acceso del iPhone.

## Si no conecta

- Comprueba que ambos dispositivos están en la misma red, Vibra está abierto y el acceso al iPhone está activado.
- Permite el acceso a la red local en los ajustes del sistema y las conexiones entrantes de Vibra si el firewall lo solicita.
- Las redes de invitados pueden aislar los dispositivos. Se necesita IPv4 local y resolución Bonjour del nombre del Mac.
- Las invitaciones anteriores a la conexión local son incompatibles. Actualiza ambas apps y genera un QR nuevo.
- Si el Mac cambia de nombre en la red, genera una nueva vinculación.

## Desarrollo

```sh
./Scripts/verify.sh
./Scripts/verify_remote.sh
./Scripts/build_ios.sh simulator
./Scripts/build_ios.sh device
./Scripts/test_remote.sh
```

El último comando compila y abre el Mac y el simulador de iPhone para una prueba manual. `VIBRA_SIMULATOR_ID` permite elegir el simulador. La firma para distribuir en un dispositivo físico se configura en Xcode con el equipo de desarrollo correspondiente.

El transporte usa WebSocket local en el puerto 8788 y cifrado Noise entre dispositivos. No requiere abrir puertos en el router. Consulta el [contrato y los límites](../../services/README.md).

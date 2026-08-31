// Helion HLS blinky
void blinky(bool *led) {
    static bool q = 0;
    q = !q;
    *led = q;
}

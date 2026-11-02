/*
------------------------------------------------------------------------------
isaac64.cpp: A Fast cryptographic random number generator
           for 32-bit and 64-bit machines.
By Bob Jenkins, 1996.  Public Domain.

Modified for modularity by Tom Bartol and Rex Kerr
------------------------------------------------------------------------------
*/
#include <stdint.h>
#include <stddef.h>
#include "Isaac64.h"
typedef uint64_t ub8;
typedef uint32_t ub4;
typedef uint8_t ub1;
#define RANDSIZL 8
#define RANDSIZ (1 << RANDSIZL)
#define RANDMAX (2 * RANDSIZ)
struct isaac64_state { int randcnt; ub8 aa, bb, cc; ub8 randrsl[RANDSIZ], mm[RANDSIZ]; };


#define ind(mm, x) (*(ub8*) ((ub1*) (mm) + ((x) & ((RANDSIZ - 1) << 3))))

#define rngstep(mix, a, b, mm, m, m2, r, x)      \
    {                                            \
        x = *m;                                  \
        a = (mix) + *(m2++);                     \
        *(m++) = y = ind(mm, x) + a + b;         \
        *(r++) = b = ind(mm, y >> RANDSIZL) + x; \
    }

#define mix(a, b, c, d, e, f, g, h) \
    {                               \
        a -= e;                     \
        f ^= h >> 9;                \
        h += a;                     \
        b -= f;                     \
        g ^= a << 9;                \
        a += b;                     \
        c -= g;                     \
        h ^= b >> 23;               \
        b += c;                     \
        d -= h;                     \
        a ^= c << 15;               \
        c += d;                     \
        e -= a;                     \
        b ^= d >> 14;               \
        d += e;                     \
        f -= b;                     \
        c ^= e << 20;               \
        e += f;                     \
        g -= c;                     \
        d ^= f >> 17;               \
        f += g;                     \
        h -= d;                     \
        e ^= g << 14;               \
        g += h;                     \
    }


void isaac64_generate(struct isaac64_state* rng) {
    ub8 a, b, x, y, *m, *m2, *r, *mend;

    m = rng->mm;
    r = rng->randrsl;
    a = rng->aa;
    b = rng->bb + (++rng->cc);
    for (m = rng->mm, mend = m2 = m + (RANDSIZ / 2); m < mend;) {
        rngstep(~(a ^ (a << 21)), a, b, rng->mm, m, m2, r, x);
        rngstep(a ^ (a >> 5), a, b, rng->mm, m, m2, r, x);
        rngstep(a ^ (a << 12), a, b, rng->mm, m, m2, r, x);
        rngstep(a ^ (a >> 33), a, b, rng->mm, m, m2, r, x);
    }
    for (m2 = rng->mm; m2 < mend;) {
        rngstep(~(a ^ (a << 21)), a, b, rng->mm, m, m2, r, x);
        rngstep(a ^ (a >> 5), a, b, rng->mm, m, m2, r, x);
        rngstep(a ^ (a << 12), a, b, rng->mm, m, m2, r, x);
        rngstep(a ^ (a >> 33), a, b, rng->mm, m, m2, r, x);
    }
    rng->bb = b;
    rng->aa = a;
}


void isaac64_init(struct isaac64_state* rng, ub8 seed) {
    ub8 *r, *m;
    ub8 a, b, c, d, e, f, g, h;
    ub4 i;

    rng->aa = (ub8) 0;
    rng->bb = (ub8) 0;
    rng->cc = (ub8) 0;

    a = b = c = d = e = f = g = h = 0x9e3779b97f4a7c13LL; /* the golden ratio */

    r = rng->randrsl;
    m = rng->mm;

    for (i = 0; i < RANDSIZ; ++i)
        r[i] = (ub8) 0;

    r[0] = seed;

    for (i = 0; i < 4; ++i) /* scramble it */
    {
        mix(a, b, c, d, e, f, g, h);
    }

    for (i = 0; i < RANDSIZ; i += 8) /* fill in m[] with messy stuff */
    {
        /* use all the information in the seed */
        a += r[i];
        b += r[i + 1];
        c += r[i + 2];
        d += r[i + 3];
        e += r[i + 4];
        f += r[i + 5];
        g += r[i + 6];
        h += r[i + 7];
        mix(a, b, c, d, e, f, g, h);
        m[i] = a;
        m[i + 1] = b;
        m[i + 2] = c;
        m[i + 3] = d;
        m[i + 4] = e;
        m[i + 5] = f;
        m[i + 6] = g;
        m[i + 7] = h;
    }

    /* do a second pass to make all of the seed affect all of m[] */
    for (i = 0; i < RANDSIZ; i += 8) {
        a += m[i];
        b += m[i + 1];
        c += m[i + 2];
        d += m[i + 3];
        e += m[i + 4];
        f += m[i + 5];
        g += m[i + 6];
        h += m[i + 7];
        mix(a, b, c, d, e, f, g, h);
        m[i] = a;
        m[i + 1] = b;
        m[i + 2] = c;
        m[i + 3] = d;
        m[i + 4] = e;
        m[i + 5] = f;
        m[i + 6] = g;
        m[i + 7] = h;
    }

    isaac64_generate(rng);  /* fill in the first set of results */
    rng->randcnt = RANDMAX; /* prepare to use the first set of results */
}

/* WeChat uses the 64-bit seed, 256-word state and reverse-order big-endian output. */
void shandian_wechat_xor(uint64_t seed, uint8_t *bytes, size_t length) {
    struct isaac64_state rng;
    isaac64_init(&rng, seed);
    size_t count = length < 131072 ? length : 131072;
    size_t offset = 0;
    int remaining = 255;
    while (offset < count) {
        uint64_t word = rng.randrsl[remaining];
        if (remaining == 0) { isaac64_generate(&rng); remaining = 255; } else { --remaining; }
        for (int shift = 56; shift >= 0 && offset < count; shift -= 8) bytes[offset++] ^= (uint8_t)(word >> shift);
    }
}

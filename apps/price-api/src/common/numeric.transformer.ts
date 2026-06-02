import { ValueTransformer } from 'typeorm';

/**
 * TypeORM returns `numeric`/`decimal` columns as strings to preserve precision.
 * This transformer exposes them as JS numbers on the entity while keeping the
 * exact decimal type in Postgres. Prices here are small USD amounts where f64
 * precision is more than sufficient.
 */
export class NumericTransformer implements ValueTransformer {
  to(value: number | null): number | null {
    return value;
  }

  from(value: string | null): number | null {
    if (value === null || value === undefined) {
      return null;
    }
    return parseFloat(value);
  }
}

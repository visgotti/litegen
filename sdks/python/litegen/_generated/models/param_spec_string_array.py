from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ParamSpecStringArray")


@_attrs_define
class ParamSpecStringArray:
    r"""A param whose value is a SET of enum members rather than one of them.

    Exists for `output_formats`, where the vendors genuinely disagree on
    cardinality: Meshy emits every requested container from one job, Rodin's
    `geometry_file_format` is a scalar so only one is reachable, and Stability
    emits GLB with no choice at all. `max_items` is how a model states which of
    those it is, so an unsatisfiable request fails in validation rather than
    after the vendor has been billed.

    For `output_formats` the catalog yaml and the API describe two different
    sets, on purpose:

    - **yaml** `enum_values` declares what the VENDOR emits natively — the only
      thing a catalog author can verify against the vendor's schema.
    - **`GET /v1/models/{id}`** serves what a CALLER may request: the native
      set plus every container litegen derives locally, with `derivable_values`
      saying which those are. That is the same set the validator accepts, so a
      client rendering `params` offers exactly what will be honoured.

        Attributes:
            default (Union[Unset, List[str]]): Applied when the caller omits the param entirely. Empty means the
                resolver falls back to `glb`.
            derivable_values (Union[Unset, List[str]]): The members litegen produces locally from a single vendor job (for
                `output_formats`, whatever its mesh converter covers — today `glb`,
                `obj`, `stl`, `ply`). Present only in API
                responses, and only when the model's vendor emits at least one of them —
                with nothing readable to convert from, nothing is derivable. Computed
                by the server; the catalog loader refuses a yaml that sets it.
            description (Union[None, Unset, str]):
            enum_values (Union[Unset, List[str]]): Accepted members. See the type docs for what this means in yaml versus
                in an API response.
            label (Union[None, Unset, str]):
            max_items (Union[None, Unset, int]): For `output_formats`: how many VENDOR jobs one request may need — not
                how many members it may name. Every member of `derivable_values` comes
                out of one source container, so a request costs
                `|requested \ derivable_values| + (requested ∩ derivable_values ≠ ∅ ? 1 : 0)`
                jobs, and more than `max_items` is refused with `param_too_many`.
                `None` means no limit.
    """

    default: Union[Unset, List[str]] = UNSET
    derivable_values: Union[Unset, List[str]] = UNSET
    description: Union[None, Unset, str] = UNSET
    enum_values: Union[Unset, List[str]] = UNSET
    label: Union[None, Unset, str] = UNSET
    max_items: Union[None, Unset, int] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        default: Union[Unset, List[str]] = UNSET
        if not isinstance(self.default, Unset):
            default = self.default

        derivable_values: Union[Unset, List[str]] = UNSET
        if not isinstance(self.derivable_values, Unset):
            derivable_values = self.derivable_values

        description: Union[None, Unset, str]
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        enum_values: Union[Unset, List[str]] = UNSET
        if not isinstance(self.enum_values, Unset):
            enum_values = self.enum_values

        label: Union[None, Unset, str]
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        max_items: Union[None, Unset, int]
        if isinstance(self.max_items, Unset):
            max_items = UNSET
        else:
            max_items = self.max_items

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if default is not UNSET:
            field_dict["default"] = default
        if derivable_values is not UNSET:
            field_dict["derivable_values"] = derivable_values
        if description is not UNSET:
            field_dict["description"] = description
        if enum_values is not UNSET:
            field_dict["enum_values"] = enum_values
        if label is not UNSET:
            field_dict["label"] = label
        if max_items is not UNSET:
            field_dict["max_items"] = max_items

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        default = cast(List[str], d.pop("default", UNSET))

        derivable_values = cast(List[str], d.pop("derivable_values", UNSET))

        def _parse_description(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        description = _parse_description(d.pop("description", UNSET))

        enum_values = cast(List[str], d.pop("enum_values", UNSET))

        def _parse_label(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        label = _parse_label(d.pop("label", UNSET))

        def _parse_max_items(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        max_items = _parse_max_items(d.pop("max_items", UNSET))

        param_spec_string_array = cls(
            default=default,
            derivable_values=derivable_values,
            description=description,
            enum_values=enum_values,
            label=label,
            max_items=max_items,
        )

        param_spec_string_array.additional_properties = d
        return param_spec_string_array

    @property
    def additional_keys(self) -> List[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
